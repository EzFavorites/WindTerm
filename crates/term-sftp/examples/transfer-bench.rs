//! 传输基准驱动：本地 `fs::copy` 基线 vs **本项目** `term-sftp` 的上传 / 下载。
//!
//! 为什么不用系统 `sftp` 命令：`protocol-verify.sh` 用系统客户端验的是「对端协议兼容」，
//! 而基准要回答的是「我们自己的传输实现有多快」。用系统客户端测出来的数字与本仓代码无关。
//!
//! 不挂 `verify.sh` 的 step（那会把 `verify_steps` 从 38 改成 39，连带 `facts.txt`），
//! 由 `scripts/transfer-bench.sh` 起容器后显式调用。连接参数全部走
//! `WINDTERM_DOCKER_*`，与 `crates/term-ssh/tests/docker_sshd.rs` 同一套约定。

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use term_sftp::blocking::Client;
use term_ssh::{AuthMethod, HostKeyPolicy, SshOptions};

const CHUNK: usize = 1024 * 1024;
/// 小文件组的单个载荷：4 KiB，足以越过 SFTP 分块的小端，又不至于把基准拖长。
const SMALL_FILE_BYTES: usize = 4 * 1024;

/// 取连接参数。返回错误而不是 `panic!`：4p（生产路径 panic 冻结，只许减不许增）
/// 也扫 `crates/` 下的示例，而「缺环境变量」是**输入**问题，给一行能照做的
/// 提示比栈回溯有用。
fn var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| {
        format!("缺少环境变量 {name}：先跑 bash scripts/transfer-bench.sh，它会起容器并导出参数")
    })
}

fn mb_per_sec(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    bytes as f64 / (1024.0 * 1024.0) / secs
}

/// 不引第三方哈希 crate：FNV-1a 够用来判「传完的字节和源一样」。
/// 基准要的不是密码学强度，而是零依赖地发现截断 / 错位。
fn fingerprint(path: &Path) -> std::io::Result<u64> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = [0u8; 64 * 1024];
    let mut reader = BufReader::new(File::open(path)?);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for byte in &buf[..n] {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    Ok(hash)
}

/// 载荷是确定的字节图案，不靠 `/dev/urandom`：同一尺寸每轮内容相同，
/// 才能把「本地 / 上传 / 下载」三条曲线放在同一份数据上比较。
fn make_payload(dir: &Path, size_mb: u64) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("payload-{size_mb}mib.bin"));
    if !path.exists() || fs::metadata(&path)?.len() != size_mb * CHUNK as u64 {
        let mut block = vec![0u8; CHUNK];
        for (i, byte) in block.iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }
        let mut file = File::create(&path)?;
        for _ in 0..size_mb {
            use std::io::Write;
            file.write_all(&block)?;
        }
    }
    Ok(path)
}

fn options() -> Result<SshOptions, Box<dyn std::error::Error>> {
    let mut opts = SshOptions::new(
        &var("WINDTERM_DOCKER_HOST")?,
        &var("WINDTERM_DOCKER_USER")?,
        AuthMethod::Password {
            password: var("WINDTERM_DOCKER_PASS")?,
        },
    );
    // 端口不是数字是**输入**问题：`ParseIntError` 自己就带「哪个值、为什么」，
    // 原样上抛比 `.expect` 更清楚。
    opts.port = var("WINDTERM_DOCKER_PORT")?.parse()?;
    // 基准只关心字节 throughput，host key 校验已由 docker_sshd.rs 单独验过。
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts.keepalive = None;
    Ok(opts)
}

struct Row {
    local: f64,
    put: f64,
    get: f64,
    ok: bool,
}

/// 小文件组的一行结果。三段各自计时：put / get 报 MB/s 与 files/s，
/// tree-delete 报「每秒删掉多少步」（每次删除都是一次协议往返）。
struct SmallRow {
    count: usize,
    bytes: u64,
    put_secs: f64,
    get_secs: f64,
    delete_secs: f64,
    put: f64,
    get: f64,
    delete_ops_per_sec: f64,
    steps: usize,
    ok: bool,
}

/// 跑一组「N 个小文件 put / get / tree-delete」。
///
/// 串行是**刻意的**：与界面同一条会话、同一套请求顺序，所以「先删文件后后序
/// 删目录」在这里是被真服务端验过的，而不是只跑过纯函数单测。并行化要改
/// `xfer_queue` 的 `max_parallel` 与设计文档，本轮保留差异，见
/// `docs/page6-transfer-notes.md`。
fn run_small_bench(
    client: &Client,
    work: &Path,
    count: usize,
) -> Result<SmallRow, Box<dyn std::error::Error>> {
    let local_root = work.join("small-src");
    // 清掉上一次残留（含 count 变小时多出来的文件），每轮形状一致。
    let _ = fs::remove_dir_all(&local_root);
    fs::create_dir_all(&local_root)?;
    let mut names = Vec::with_capacity(count);
    let mut hashes = Vec::with_capacity(count);
    for i in 0..count {
        let name = format!("f-{i:05}.bin");
        let mut buf = vec![0u8; SMALL_FILE_BYTES];
        for (j, byte) in buf.iter_mut().enumerate() {
            *byte = ((i + j) % 251) as u8;
        }
        let path = local_root.join(&name);
        fs::write(&path, &buf)?;
        hashes.push(fingerprint(&path)?);
        names.push(name);
    }

    // 远端根带 pid：上一次异常中断留下的目录不会混进这次的计时。
    let remote = format!("{}/windterm-bench-small-{}", client.home(), std::process::id());
    // 已存在就接着用：同名文件会被 put 覆盖，多的旧文件在 tree-delete 里一起收走。
    // 真建不出来（权限）时 put 会给出带路径的原因，不必在这里抢着报错。
    let _ = client.mkdir(&remote);

    let bytes = SMALL_FILE_BYTES as u64 * count as u64;
    let t = Instant::now();
    for name in &names {
        client.put(
            &local_root.join(name).to_string_lossy(),
            &format!("{remote}/{name}"),
        )?;
    }
    let put_elapsed = t.elapsed();
    let put = mb_per_sec(bytes, put_elapsed);

    let down_root = work.join("small-dst");
    let _ = fs::remove_dir_all(&down_root);
    fs::create_dir_all(&down_root)?;
    let t = Instant::now();
    for name in &names {
        client.get(
            &format!("{remote}/{name}"),
            &down_root.join(name).to_string_lossy(),
        )?;
    }
    let get_elapsed = t.elapsed();
    let get = mb_per_sec(bytes, get_elapsed);

    // 用产品同款纯计划 + 既有删除请求收尾：这一步同时是「树删除真能删干净」
    // 的服务端验证，不是只跑过单测的纸面顺序。
    let listed = client.list(&remote)?;
    let entries: Vec<term_sftp::ScannedEntry> = listed
        .iter()
        .map(|e| term_sftp::ScannedEntry {
            rel: e.name.clone(),
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect();
    let steps = term_sftp::plan_tree_delete(&remote, &entries)?;
    let t = Instant::now();
    for step in &steps {
        match step {
            term_sftp::DeleteStep::File(path) => client.remove_file(path)?,
            term_sftp::DeleteStep::Dir(path) => client.remove_dir(path)?,
        }
    }
    let delete_elapsed = t.elapsed();
    let ok = hashes
        .iter()
        .zip(names.iter())
        .all(|(want, name)| {
            fingerprint(&down_root.join(name))
                .map(|got| got == *want)
                .unwrap_or(false)
        })
        && client.stat(&remote).is_err();
    let secs = |d: Duration| d.as_secs_f64().max(f64::MIN_POSITIVE);
    Ok(SmallRow {
        count,
        bytes,
        put_secs: secs(put_elapsed),
        get_secs: secs(get_elapsed),
        delete_secs: secs(delete_elapsed),
        put,
        get,
        delete_ops_per_sec: steps.len() as f64 / secs(delete_elapsed),
        steps: steps.len(),
        ok,
    })
}

fn arg(name: &str, default: &str) -> String {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == name {
            return args.next().unwrap_or_else(|| default.to_string());
        }
    }
    default.to_string()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let size_mb: u64 = arg("--size-mb", "8").parse()?;
    let rounds: usize = arg("--rounds", "3").parse()?;
    let small_count: usize = arg("--small-count", "128").parse()?;
    let out = PathBuf::from(arg("--out", "/tmp/windterm-transfer-report.md"));
    let work = PathBuf::from(arg("--work-dir", "/tmp/windterm-transfer-bench"));
    if size_mb == 0 || rounds == 0 || small_count == 0 {
        return Err("--size-mb / --rounds / --small-count 都必须 > 0".into());
    }

    let src = make_payload(&work, size_mb)?;
    let source_hash = fingerprint(&src)?;
    let local_copy = work.join("local-copy.bin");
    let downloaded = work.join("downloaded.bin");

    let client = Client::connect(options()?)?;
    let remote = format!("{}/windterm-bench-{size_mb}mib.bin", client.home());

    let mut rows = Vec::with_capacity(rounds);
    for round in 1..=rounds {
        let t = Instant::now();
        fs::copy(&src, &local_copy)?;
        let local = mb_per_sec(size_mb * CHUNK as u64, t.elapsed());

        let t = Instant::now();
        let put_bytes = client.put(
            &src.to_string_lossy(),
            &remote,
        )?;
        let put = mb_per_sec(put_bytes, t.elapsed());

        let t = Instant::now();
        let get_bytes = client.get(&remote, &downloaded.to_string_lossy())?;
        let get = mb_per_sec(get_bytes, t.elapsed());

        let ok = fingerprint(&local_copy)? == source_hash
            && fingerprint(&downloaded)? == source_hash
            && put_bytes == size_mb * CHUNK as u64
            && get_bytes == put_bytes;
        rows.push(Row { local, put, get, ok });
        eprintln!("[transfer-bench] round {round}/{rounds}: local {local:.2} / put {put:.2} / get {get:.2} MB/s, ok={ok}");
    }
    eprintln!("[transfer-bench] small files: N={small_count} × {} B", SMALL_FILE_BYTES);
    let small = run_small_bench(&client, &work, small_count)?;
    eprintln!(
        "[transfer-bench] small: put {:.2} MB/s / get {:.2} MB/s / tree-delete {:.1} ops/s, ok={}",
        small.put, small.get, small.delete_ops_per_sec, small.ok
    );
    let _ = client; // Drop 里挂断；显式引用避免被提前优化掉。

    let mean = |f: fn(&Row) -> f64| rows.iter().map(f).sum::<f64>() / rows.len() as f64;
    // 只报均值会骗人：回环 + 页缓存的抖动能有 2–4 倍（实测 32 MiB 时 put 在
    // 57–229 MB/s 之间跳）。中位数代表「典型一轮」，极差代表「这台机器有多吵」，
    // 三个数一起看才知道这个数字能不能用来判断回归。
    let median = |f: fn(&Row) -> f64| {
        let mut v: Vec<f64> = rows.iter().map(f).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v[v.len() / 2]
    };
    let spread = |f: fn(&Row) -> f64| {
        let mut v: Vec<f64> = rows.iter().map(f).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (v[0], v[v.len() - 1])
    };
    let mut report = String::new();
    writeln!(report, "## 传输基准（本地 vs 本项目 SFTP）")?;
    writeln!(report)?;
    let (host, port, user) = (
        var("WINDTERM_DOCKER_HOST")?,
        var("WINDTERM_DOCKER_PORT")?,
        var("WINDTERM_DOCKER_USER")?,
    );
    writeln!(report, "- 对端：{host}:{port} / {user}")?;
    writeln!(
        report,
        "- 环境：{} {}（回环，Docker 容器对端；数字只在本机内有意义）",
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    writeln!(report, "- 载荷：{size_mb} MiB × {rounds} 轮（确定性字节图案，非随机）")?;
    writeln!(report, "- 被测：本地 `fs::copy` / `term_sftp::blocking::Client::put` / `::get`")?;
    writeln!(report, "- 校验：FNV-1a 指纹与源文件一致，且收发的字节数相等")?;
    writeln!(report)?;
    writeln!(report, "| 轮次 | 本地 copy MB/s | SFTP put MB/s | SFTP get MB/s | put / 本地 | get / 本地 | 校验 |")?;
    writeln!(report, "|---:|---:|---:|---:|---:|---:|:---:|")?;
    for (i, r) in rows.iter().enumerate() {
        writeln!(
            report,
            "| {} | {:.2} | {:.2} | {:.2} | {:.2}x | {:.2}x | {} |",
            i + 1,
            r.local, r.put, r.get,
            r.put / r.local, r.get / r.local,
            if r.ok { "✅" } else { "❌" }
        )?;
    }
    let (ml, mp, mg) = (mean(|r| r.local), mean(|r| r.put), mean(|r| r.get));
    writeln!(
        report,
        "| 均值 | {ml:.2} | {mp:.2} | {mg:.2} | {:.2}x | {:.2}x | — |",
        mp / ml, mg / ml
    )?;
    let (dl, dp, dg) = (median(|r| r.local), median(|r| r.put), median(|r| r.get));
    writeln!(
        report,
        "| 中位数 | {dl:.2} | {dp:.2} | {dg:.2} | {:.2}x | {:.2}x | — |",
        dp / dl, dg / dl
    )?;
    let (ll, lp, lg) = (spread(|r| r.local), spread(|r| r.put), spread(|r| r.get));
    writeln!(
        report,
        "| 极差 | {:.0}–{:.0} | {:.0}–{:.0} | {:.0}–{:.0} | — | — | — |",
        ll.0, ll.1, lp.0, lp.1, lg.0, lg.1
    )?;
    writeln!(report)?;
    writeln!(report, "比值 >1 表示经 SFTP 比本地复制更快（通常是页缓存命中差异），<1 是正常开销；跨机器 / 跨文件系统比较没有意义。")?;
    writeln!(report)?;
    writeln!(
        report,
        "判回归看**中位数**而不是均值：均值会被某一轮的系统抖动拖偏（本机实测极差可达 4 倍）。\
         改进前后的两份报告要在同一台机器、同一载荷下比，且差异超过极差才算真差异。"
    )?;

    writeln!(report)?;
    writeln!(
        report,
        "## 小文件基准（{} 个 × {} KiB，put / get / tree-delete）",
        small.count,
        SMALL_FILE_BYTES / 1024
    )?;
    writeln!(report)?;
    writeln!(report, "- 对端：{host}:{port} / {user}；同一条会话、**串行**（`xfer_queue.max_parallel = 1` 未改）")?;
    writeln!(
        report,
        "- 载荷：{} 个确定性小文件，合计 {} KiB；FNV-1a 逐文件核对，并确认树根已删掉",
        small.count,
        small.bytes / 1024
    )?;
    writeln!(report, "- 被测：`Client::put` / `::get` 逐文件；tree-delete 走 `term_sftp::plan_tree_delete` 生成的既有 `remove_file` / `remove_dir` 步骤")?;
    writeln!(report)?;
    writeln!(report, "| 阶段 | 用时 | 吞吐 | 校验 |")?;
    writeln!(report, "|:---|---:|---:|:---:|")?;
    writeln!(
        report,
        "| put（{} files） | {:.3}s | {:.2} MB/s（{:.1} files/s） | {} |",
        small.count,
        small.put_secs,
        small.put,
        small.count as f64 / small.put_secs,
        if small.ok { "✅" } else { "❌" }
    )?;
    writeln!(
        report,
        "| get（{} files） | {:.3}s | {:.2} MB/s（{:.1} files/s） | {} |",
        small.count,
        small.get_secs,
        small.get,
        small.count as f64 / small.get_secs,
        if small.ok { "✅" } else { "❌" }
    )?;
    writeln!(
        report,
        "| tree-delete（{} steps） | {:.3}s | {:.1} ops/s | {} |",
        small.steps,
        small.delete_secs,
        small.delete_ops_per_sec,
        if small.ok { "✅" } else { "❌" }
    )?;
    writeln!(report)?;
    writeln!(report, "小文件与小目录的瓶颈是**每步一次协议往返**，MB/s 会远低于上面的大载荷数字，这是正常现象；判回归请比较 files/s 与 ops/s。")?;
    writeln!(report, "并行化会牵动 `xfer_queue` 的 `max_parallel` 与设计文档，本轮不做，登记在 `docs/page6-transfer-notes.md`。")?;

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out, &report)?;
    println!("{}", out.display());
    Ok(())
}
