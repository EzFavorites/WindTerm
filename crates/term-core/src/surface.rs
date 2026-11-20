//! 会话控制面的**描述层**（ADR-0019 + ADR-0023）：方法名、一句话说明、
//! 参数与结果的结构、以及该方法对**哪个前端**可见，全部是**纯数据**。
//!
//! 为什么描述层要在 `term-core`：外部客户端（JSON-RPC 桥、agent、脚本）需要
//! 知道自己能调什么，而这份答案只能有**一处**真相。放在实现层（桥接 crate）里，
//! 每个新客户端都会抄一份、然后各自漂移；放在这里，`describe` 的回答、
//! Python 桩生成的签名、脚本引擎的动词表、文档里的清单都是同一张表的下游产物。
//!
//! # ADR-0023：一张表，两个前端
//!
//! 本表不区分「远程 API」与「脚本 API」两套动词——那是同一组会话操作的两种
//! 呈现方式。[`Exposure`] 只标注「这个动词该出现在哪个前端的动词表里」，
//! 不制造第二份语义。判据：
//!
//! - 两个前端都能用它 → [`Exposure::Both`]（绝大多数会话动词）；
//! - 只有远程需要（当前只有 `describe`：脚本侧拿不到动词表也不需要）→ `Rpc`；
//! - 只有脚本需要（例如 `cursor_pos`，远程从 `screen` 的结果字段就能拿到
//!   同样的两个数，单列一个方法只是多一次往返）→ `Script`。
//!
//! 漂移防线不在本文件，而在两个前端各自的**集合相等断言**（见 ADR-0023 §决策 3）：
//! [`names`] / [`script_names`] 是两侧的投影，测试拿它跟实际注册的动词对账。
//!
//! 纪律：本模块是数据，不是机制。
//!
//! - 零全局状态：没有懒初始化单例、没有跨线程的互斥锁 / 读写锁 / 原子指针
//!   （`scripts/verify.sh` 第 4c 项复核），`METHODS` 是一个 `static` 引用，只读。
//!   （这条注释刻意不写那些类型的名字：4c 是文本 grep，写成代码标识符会把
//!   注释本身判红。）
//! - 零 IO、零墙钟（第 4b 项复核）：schema 是 `&'static str` 常量，不在运行时
//!   读文件、不在运行时拼字符串。
//! - 不引用任何协议概念（§B.14）：没有 ssh / sftp / telnet 字样。
//!
//! schema 用的是 **JSON Schema 的一个子集**（`type` / `properties` / `required` /
//! `items` / `enum` / `description` / `minimum`）。够描述这条能力面，也够
//! 任何语言的客户端做参数校验；刻意不追求完整 JSON Schema——那会引进来一个
//! 校验器依赖，而客户端本来就有自己的参数检查。

use serde::Serialize;

/// 方法在能力面上的**类别**。
///
/// 类别不是装饰：客户端据此决定「点一下就行」还是「要等、要确认」——
/// 写操作可能改变远端状态，读操作是安全的，生命周期操作会动会话集合，
/// 流式操作会**消耗**输出。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodKind {
    /// 自省：回答「有哪些方法」（`describe`）。不碰任何会话。
    Introspect,
    /// 读：只取事实，不改变会话状态。
    Read,
    /// 写：往会话写字节。`write` / `keys` 只保证写进了本地通道，不保证远端已处理。
    Write,
    /// 会话生命周期：新建 / 关闭会话。改动控制面的登记表。
    SessionLifecycle,
    /// 流：**推进捕获游标**。重复调用会吃掉上次没读完的数据——这是语义，
    /// 不是 bug；客户端不得把流式动词当成幂等读来重试（ADR-0023 §决策 4）。
    Stream,
}

/// 动词对哪个前端可见（ADR-0023）。
///
/// 这不是「两套 API」的开关，而是「同一个动词在两个前端的呈现差异」的标注：
/// 加一个 `Script` 动词不会在远程面产生第二套语义，只是那个动词本来就不该
/// 出现在远程的动词表里。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exposure {
    /// 只对 JSON-RPC 桥可见（当前只有自省类的 `describe`）。
    Rpc,
    /// 只对脚本引擎可见（远程有等价的更省往返的表达，不必单列）。
    Script,
    /// 两个前端都可见。
    Both,
}

impl Exposure {
    /// 远程前端是否看得见。
    pub fn on_rpc(self) -> bool {
        matches!(self, Exposure::Rpc | Exposure::Both)
    }

    /// 脚本前端是否看得见。
    pub fn on_script(self) -> bool {
        matches!(self, Exposure::Script | Exposure::Both)
    }
}

/// 一个方法的自描述条目。
///
/// `summary` 直接喂给人和模型（prompt 友好）：一句话说清**做什么**以及
/// **容易踩的语义边界**，不写「TODO」「见文档」这类空话。
/// `params_schema` / `result_schema` 是合法 JSON 文本常量（`docs` 与测试
/// 都会 `serde_json::from_str` 一遍，写坏了立刻红）。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct MethodSpec {
    /// 线上方法名（JSON-RPC 的 `method` 字段 / Rhai 全局函数名）。
    pub name: &'static str,
    /// 一句话说明，面向人与模型。
    pub summary: &'static str,
    /// 参数的 JSON Schema（对象），合法 JSON 文本。
    pub params_schema: &'static str,
    /// 结果的 JSON Schema，合法 JSON 文本。
    pub result_schema: &'static str,
    /// 方法类别。
    pub kind: MethodKind,
    /// 对哪个前端可见（ADR-0023）。
    pub exposure: Exposure,
}

/// 空参数对象（`describe` / `session_list` / `session_current` 这类无参方法共用）。
const NO_PARAMS: &str = r#"{"type":"object","properties":{},"required":[]}"#;

/// 只有 `session` 一个必填参数（`session_close` / `screen` / `state` /
/// `facets` / `log_*` 共用）。schema 复用常量，是为了让这些方法的参数形状
/// **不可能**各自写歪。
const SESSION_ONLY_PARAMS: &str = r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一，ADR-0024）"}},"required":["session"]}"#;

/// 一个会话描述符。`session_list` 的每一项、`session_open` / `session_use`
/// 的结果**共用**这一份 schema：开完的会话可以直接塞回清单，不需要两套键名。
/// 里面**没有 id**（ADR-0024）：寻址键是 `name`，数字 id 退回实现细节。
const SESSION_DESCRIPTOR_SCHEMA: &str = r#"{"type":"object","properties":{"name":{"type":"string","description":"会话名（寻址键，池内唯一；命令会话通常是命令本身）"},"alive":{"type":"boolean","description":"是否存活"},"exit":{"type":["string","null"],"description":"结束时的状态文本；存活时为 null"}},"required":["name","alive","exit"]}"#;


/// 常量：只返回一个字符串（`version` / `script_path` / `region` …）。
const TEXT_RESULT: &str = r#"{"type":"string"}"#;

/// 常量：`{"value": bool}`。
const FLAG_RESULT: &str = r#"{"type":"object","properties":{"value":{"type":"boolean"}},"required":["value"]}"#;

/// 常量：`{"value": 整数}`。
const COUNT_RESULT: &str = r#"{"type":"object","properties":{"value":{"type":"integer"}},"required":["value"]}"#;

/// 常量：`{"ok": true}`（失败走 error 分支）。
const OK_RESULT: &str = r#"{"type":"object","properties":{"ok":{"type":"boolean","description":"恒为 true；失败走 error 分支"}},"required":["ok"]}"#;

/// 常量：会话面扩展动词的参数（`session` 可省略 = 当前绑定）。
const SESSION_SCOPE_PARAMS: &str = r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"目标会话名；省略时用脚本当前绑定的那条"}},"required":[]}"#;

/// 能力面清单（ADR-0023：本表同时是脚本引擎与 JSON-RPC 桥的动词来源）。
///
/// 顺序即 `describe` 的回答顺序，也是 Python 桩里方法的声明顺序——
/// 刻意把 `describe` 放第一（先让客户端发现能力，再调别的）。
pub static METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "describe",
        summary: "列出桥接支持的全部方法与参数/结果结构；客户端据此发现能力，\
                  不得硬编码方法名。本表由 term-core 的 METHODS 单一真相生成。",
        params_schema: NO_PARAMS,
        result_schema: r#"{"type":"object","properties":{"methods":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"summary":{"type":"string"},"kind":{"type":"string","enum":["introspect","read","write","session_lifecycle","stream"]},"params_schema":{"type":"object"},"result_schema":{"type":"object"}},"required":["name","summary","kind","params_schema","result_schema"]}}},"required":["methods"]}"#,
        kind: MethodKind::Introspect,
        exposure: Exposure::Rpc,
    },
    MethodSpec {
        name: "session_list",
        summary: "列出当前所有会话：会话名、是否存活、退出状态文本。\
                  清单里没有的名字一律视为不存在（不要拿旧名字试探）。\
                  名字是寻址键（ADR-0024），池内唯一，所以清单里不会有重名。",
        params_schema: NO_PARAMS,
        result_schema: r#"{"type":"object","properties":{"sessions":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"alive":{"type":"boolean"},"exit":{"type":["string","null"]}},"required":["name","alive","exit"]}}},"required":["sessions"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "session_open",
        summary: "新开一个会话并返回它的描述符：给 command 就走一次性命令（经 /bin/sh -c），\
                  不给则开一个本地 shell。起不来是错误，不会返回半个会话。\
                  **返回的 name 才是寻址键**：与已有会话重名时池会自动加后缀\
                  （cat → cat-2），所以必须用返回值里的 name，不能用你传进去的那个。\
                  脚本侧新开的会话**会成为当前绑定**（session_current 随之改变）。",
        params_schema: r#"{"type":"object","properties":{"command":{"type":"string","description":"一次性命令；给了就走 exec 通道，不给则开本地 shell"},"name":{"type":"string","description":"会话名的基名（仅 command 缺省时生效；与已有会话重名时池会自动加后缀）"},"cols":{"type":"integer","minimum":1,"description":"终端列数，缺省 80"},"rows":{"type":"integer","minimum":1,"description":"终端行数，缺省 24"}},"required":[]}"#,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::SessionLifecycle,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "session_close",
        summary: "关闭并移除一个会话。名字不存在是**错误**（not_found），不是\
                  {\"closed\": false} 这样的空成功——静默成功会让调用方以为关掉了。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"closed":{"type":"boolean","description":"恒为 true；失败走 error 分支"}},"required":["closed"]}"#,
        kind: MethodKind::SessionLifecycle,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "session_current",
        summary: "查脚本当前绑定的**会话名**——没有 session 参数的原语（write / wait / \
                  read_until / log_* …）都作用在它身上。只对脚本可见：远程前端每次 \
                  调用自带 session 参数，没有「当前」这个概念。",
        params_schema: NO_PARAMS,
        result_schema: r#"{"type":"string","description":"当前绑定的会话名"}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_use",
        summary: "把当前绑定切到这个会话名，之后的无参原语都作用在它身上。只对脚本可见\
                  （远程是参数寻址，不需要绑定）。名字不存在是**错误**（not_found），\
                  不会静默留在原来那条会话上。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::SessionLifecycle,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "write",
        summary: "往会话写入**原始字节**（对提示的应答、要自己拼控制字符的场合）。\
                  只保证写进本地通道，不保证远端已处理——要确认效果就用 wait 等它的\
                  回显。要按「键」而不是按字符输入请用 keys。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"text":{"type":"string","description":"要写入的文本（按 UTF-8 转字节，原样发送，不自动加换行；要发控制字符就自己带 \\u000d 之类）"}},"required":["session","text"]}"#,
        result_schema: r#"{"type":"object","properties":{"written":{"type":"boolean","description":"恒为 true；失败走 error 分支"}},"required":["written"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "keys",
        summary: "按键名发送按键：enter / tab / esc / backspace / delete / insert / home / \
                  end / pageup / pagedown / up / down / left / right / f1..f12 / shift+tab / \
                  ctrl+a..ctrl+z / alt+<单字符>。与 write 的分工是语义而不是风格——\
                  keys 认「键」，所以 enter 是 CR、F5 是 ESC[15~、方向键按会话的 \
                  DECSET 1 选 ESC[A 或 ESC OA；write 只送字节。未知键名是**错误**，\
                  不会被当成文本写出去。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"keys":{"type":"array","items":{"type":"string"},"description":"键名列表，按顺序发送，至少一项（大小写与 -/_/+ 分隔符不敏感）"}},"required":["session","keys"]}"#,
        result_schema: r#"{"type":"object","properties":{"written":{"type":"boolean","description":"恒为 true；失败走 error 分支"},"bytes":{"type":"integer","minimum":1,"description":"实际写入的字节数（用来确认 alt+/f5 这类多字节键）"}},"required":["written","bytes"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "wait",
        summary: "登记等待直到会话输出里出现 pattern（纯文本匹配，不是正则；\
                  转义序列已被剥掉，所以 ESC[31m 挡不住 Password:）。返回自登记点到\
                  匹配串结尾的文本。超时是**错误**（timeout），不是空结果。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"pattern":{"type":"string","description":"要等待的纯文本模式，不能是空串"},"timeout_ms":{"type":"integer","minimum":0,"description":"等待上限（毫秒），缺省 30000；超时即 error(timeout)"}},"required":["session","pattern"]}"#,
        result_schema: r#"{"type":"object","properties":{"text_upto":{"type":"string","description":"自登记点到匹配串结尾的清洗后文本（给全这段而不只给匹配串）"}},"required":["text_upto"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "read",
        summary: "排水：取走自上一次读取点以来的全部输出并推进游标；没有积累时等到\
                  有新数据或超时。与 read_until 不同，超时返回空串**不是错误**——\
                  排水不是「等待某个事实」。流式动词，重复调用会吃掉中间数据。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"timeout_ms":{"type":"integer","minimum":0,"description":"等到新数据的上限（毫秒），缺省 0 = 只排水不等"}},"required":["session"]}"#,
        result_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"自上次读取点以来的全部文本；无新数据时为空串"}},"required":["text"]}"#,
        kind: MethodKind::Stream,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "read_until",
        summary: "自上次读取点往后读到 delimiter 首次出现（返回值含该 delimiter），\
                  游标推进到它结尾，之后的尾部不丢、留给下次 read。超时是**错误**\
                  （timeout），游标不动、文本不丢。流式动词，不是幂等读。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"delimiter":{"type":"string","description":"哨兵串，不能是空串"},"timeout_ms":{"type":"integer","minimum":0,"description":"等待上限（毫秒），缺省 30000"}},"required":["session","delimiter"]}"#,
        result_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"自上次读取点到 delimiter 结尾的文本（含 delimiter）"}},"required":["text"]}"#,
        kind: MethodKind::Stream,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "wait_any",
        summary: "等多个模式中任意一个出现，返回**最先出现**者的 1 起索引与该段文本。\
                  同一位置多个模式命中时取数组里靠前的。超时是**错误**，不返回索引 0；\
                  流式动词，命中后游标推进到命中串结尾。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"patterns":{"type":"array","items":{"type":"string"},"description":"要等待的模式列表，至少一项且不得含空串"},"timeout_ms":{"type":"integer","minimum":0,"description":"等待上限（毫秒），缺省 30000"}},"required":["session","patterns"]}"#,
        result_schema: r#"{"type":"object","properties":{"index":{"type":"integer","minimum":1,"description":"命中模式在 patterns 里的位置，从 1 起"},"text":{"type":"string","description":"自游标到命中串结尾的文本（含命中串）"}},"required":["index","text"]}"#,
        kind: MethodKind::Stream,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "wait_idle",
        summary: "等到**输出停下来**：连续 quiet_ms 毫秒没有新输出就返回这段时间收到的\
                  文本，并把读取游标推进到它结尾。与 wait 的分工是「等事实」vs「等安静」——\
                  提示语不确定、只知道命令会结束的场合用这个。超时（压根没安静下来）是\
                  **错误**（timeout）；流式动词，返回的文本已经被消耗。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"quiet_ms":{"type":"integer","minimum":1,"description":"多久没有新输出算「停下来」（毫秒）"},"timeout_ms":{"type":"integer","minimum":0,"description":"整体等待上限（毫秒），缺省 30000"}},"required":["session","quiet_ms"]}"#,
        result_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"安静窗口期间收到的全部文本；没有新输出时为空串"}},"required":["text"]}"#,
        kind: MethodKind::Stream,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "set_synchronous",
        summary: "打开/关闭**同步写**：开启后每次 write / keys 都会等到输出安静下来才返回，\
                  于是「写完立刻 read」不再有漏读窗口。粘性状态——只对脚本可见：远程是\
                  每次调用自带参数的寻址模型，一条会改变后续调用语义的粘性开关在那里\
                  没有地方表达（也就没法告诉客户端「你现在是同步的」）。",
        params_schema: r#"{"type":"object","properties":{"on":{"type":"boolean","description":"true 开启同步写，false 关闭"}},"required":["on"]}"#,
        result_schema: r#"{"type":"object","properties":{"synchronous":{"type":"boolean","description":"设置后的状态（与入参一致）"}},"required":["synchronous"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "screen",
        summary: "取当前屏幕快照的纯文本与行列数。纯读取：不消费输出、不推进任何游标，\
                  因此可以反复调。会话已结束且没有事实可取时报 closed，不是返回空屏。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"整屏文本，行间以 \\n 分隔"},"cols":{"type":"integer"},"rows":{"type":"integer"},"cursor_row":{"type":"integer","description":"光标所在行（0 起）；RPC 侧的光标信息从这里取"},"cursor_col":{"type":"integer","description":"光标所在列（0 起）"}},"required":["text","cols","rows"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "state",
        summary: "查会话生命周期状态：存活，或已结束及退出状态文本。\
                  已结束的会话仍在册（能用 session_close 清掉），不要把它当不存在。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"alive":{"type":"boolean"},"exit":{"type":["string","null"],"description":"结束时的退出状态文本；存活时为 null"}},"required":["alive","exit"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "facets",
        summary: "查该会话暴露的能力 facet（文件传输 / 端口转发）。空表不是错误——\
                  本地 shell 就没有文件传输能力；调用方不得按协议硬编码假设。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"facets":{"type":"array","items":{"type":"string","enum":["file_transfer","port_forward"]}}},"required":["facets"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "log_start",
        summary: "开始把该会话的输出记到 dir 目录（文件名由会话侧按时间戳+序号决定，\
                  调用方不指定文件名，否则并发日志会互相覆盖）。已经在记时是**错误**，\
                  不会静默换文件。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string","minLength":1,"description":"会话名（`session_list` 返回的 name；池内唯一）"},"dir":{"type":"string","description":"目标目录（必填：控制面不猜用户的日志目录）"}},"required":["session","dir"]}"#,
        result_schema: r#"{"type":"object","properties":{"recording":{"type":"boolean","description":"恒为 true；失败走 error 分支"},"path":{"type":["string","null"],"description":"实际写入的文件路径"}},"required":["recording"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "log_stop",
        summary: "停止记录并关闭日志文件。没在记时是**错误**（invalid_request），\
                  不是空成功——否则调用方会以为文件已经收尾了。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"recording":{"type":"boolean","description":"恒为 false；失败走 error 分支"}},"required":["recording"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "log_status",
        summary: "查会话日志的当前阶段（off / recording / paused / failed）与文件路径。\
                  failed 会带原因：写不进去时说「记上了」比报错更糟。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"phase":{"type":"string","enum":["off","recording","paused","failed"]},"path":{"type":["string","null"]},"reason":{"type":["string","null"],"description":"phase=failed 时的原因，其余为 null"}},"required":["phase","path"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Both,
    },
    MethodSpec {
        name: "cursor_pos",
        summary: "取光标所在的行与列（均 0 起）。脚本用它把「屏幕上的位置」当作后续\
                  写操作的参考系；远程客户端从 screen 的结果字段就能拿到，\
                  所以本方法只对脚本可见（ADR-0023）。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"row":{"type":"integer","minimum":0},"col":{"type":"integer","minimum":0}},"required":["row","col"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "terminal_size",
        summary: "取终端行列数。脚本用它判断会话是否被 resize 过（换行与截断都随之变）；\
                  远程客户端从 screen 的 cols/rows 就能拿到，所以只对脚本可见。",
        params_schema: SESSION_ONLY_PARAMS,
        result_schema: r#"{"type":"object","properties":{"cols":{"type":"integer","minimum":1},"rows":{"type":"integer","minimum":1}},"required":["cols","rows"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    // ── 面向人的两条 ────────────────────────────────────────────
    //
    // 它们不是会话操作，因此只在脚本前端出现：远程调用的「用户」在客户端那一侧，
    // 由服务端弹一句话没有接收者（`desk serve` 是无窗口进程，`notify` 只会在
    // 终端里打一行）。这与 `cursor_pos` 只给脚本的理由不同——那是省一次性往返，
    // 这是**根本没有那个用户**。
    MethodSpec {
        name: "notify",
        summary: "把一句话送到用户眼前（窗口里是状态栏一行，无窗口时是 stderr）。\
                  不给返回值：这是一条提示，不是可以被确认收到的通知——脚本不该\
                  因为「用户看没看见」而改自己的分支。",
        params_schema: r#"{"type":"object","properties":{"message":{"type":"string","description":"要显示的一句话，不能是空串"}},"required":["message"]}"#,
        result_schema: r#"{"type":"object","properties":{"delivered":{"type":"boolean","description":"恒为 true（消息被送出去了；不表示有人读到了）"}},"required":["delivered"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "ask",
        summary: "向用户提问并取一行的回答（提示语先写出去，再读一行）。无头运行且没有\
                  可交互的标准输入时是**错误**，不会返回空串冒充「用户按了回车」——\
                  脚本据此可以把「要人确认」的那条分支显式跳过去，而不是静默走「确认了」。",
        params_schema: r#"{"type":"object","properties":{"prompt":{"type":"string","description":"显示给用户的提示语，不能是空串"}},"required":["prompt"]}"#,
        result_schema: r#"{"type":"object","properties":{"answer":{"type":"string","description":"用户输入的一行（已去掉行尾换行；可能是空串）"}},"required":["answer"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "version",
        summary: "返回程序版本串（`WindTerm <版本>`）。远程与脚本同一份来源：包版本，不是界面上的营销号。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "script_path",
        summary: "返回当前脚本的文件路径；从标准输入（`-`）运行时为空串。用它做「相对自己的目录找数据文件」这类事。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "args",
        summary: "返回命令行参数数组（`desk script run --arg a --arg b`）。越界索引返回空串，不报错——CRT 的 Arguments 同语义。",
        params_schema: r#"{"type":"object","properties":{"index":{"type":"integer","minimum":0,"description":"可选：取第几项（0 起）；不给则返回整个数组"}},"required":[]}"#,
        result_schema: r#"{"type":["array","string"],"description":"整个数组，或指定索引的那一项（越界为空串）"}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "last_error",
        summary: "返回最近一次脚本错误的文本（没有错误时为空串）。脚本据此在收尾时汇报「到底哪一步坏了」。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "script_session",
        summary: "返回脚本启动时绑定的会话名。与 session_current 的区别：后者会随 session_use 变，这个不变。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_at",
        summary: "按会话集合里的 1 起序号取一条会话描述（与标签栏顺序一致）。越界是错误，不会返回最后一条。",
        params_schema: r#"{"type":"object","properties":{"index":{"type":"integer","minimum":1,"description":"1 起序号"}},"required":["index"]}"#,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_index",
        summary: "返回当前绑定会话在集合里的 1 起序号。宿主没有稳定顺序时报错（不返回 0 冒充第一条）。",
        params_schema: NO_PARAMS,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_active",
        summary: "返回界面当前激活的会话描述。无界面宿主退回脚本自己的绑定——也就是说这个动词永远有答案。",
        params_schema: NO_PARAMS,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_info",
        summary: "返回一条会话的完整描述：name / alive / exit / phase / local_addr / host。省略会话名时取当前绑定。",
        params_schema: SESSION_SCOPE_PARAMS,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_connect",
        summary: "按会话库里的名字打开一条会话并绑定它（CRT 的 Session.Connect）。密码认证需要保险库，无窗口运行会显式失败。",
        params_schema: r#"{"type":"object","properties":{"name":{"type":"string","minLength":1,"description":"会话库里的名字"}},"required":["name"]}"#,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::SessionLifecycle,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_clone",
        summary: "复制一条会话（复制配置、名字加后缀、复用保险库凭据），副本成为当前绑定。需要界面宿主。",
        params_schema: r#"{"type":"object","properties":{"name":{"type":"string","minLength":1,"description":"要复制的会话名"}},"required":["name"]}"#,
        result_schema: SESSION_DESCRIPTOR_SCHEMA,
        kind: MethodKind::SessionLifecycle,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "session_activate",
        summary: "把一条会话带到前台（脚本的当前绑定不变）。无界面宿主显式报不支持。",
        params_schema: r#"{"type":"object","properties":{"name":{"type":"string","minLength":1,"description":"要激活的会话名"}},"required":["name"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sleep",
        summary: "等待若干毫秒，可被「停止」打断；不消费会话输出（收下的文本留给后续 read，与 CRT 的 Sleep 一致）。",
        params_schema: r#"{"type":"object","properties":{"millis":{"type":"integer","minimum":0,"description":"毫秒数"}},"required":["millis"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "send_keys",
        summary: "按 WSH SendKeys 语法发按键：^C 是 Ctrl+C、{F7} 是功能键、+{TAB} 是 Shift+Tab、~ 是回车。认不出的键名是错误。",
        params_schema: r#"{"type":"object","properties":{"expr":{"type":"string","description":"SendKeys 表达式"}},"required":["expr"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "region",
        summary: "取屏幕矩形文本，坐标 1 起、含端点、越界裁剪；行间用 CRLF（CRT Screen.Get 的形状）。纯读取，不推进游标。",
        params_schema: r#"{"type":"object","properties":{"row1":{"type":"integer","minimum":1},"col1":{"type":"integer","minimum":1},"row2":{"type":"integer","minimum":1},"col2":{"type":"integer","minimum":1}},"required":["row1","col1","row2","col2"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "region_lines",
        summary: "同 region，但行间用 LF，便于脚本按行 split 逐行处理。",
        params_schema: r#"{"type":"object","properties":{"row1":{"type":"integer","minimum":1},"col1":{"type":"integer","minimum":1},"row2":{"type":"integer","minimum":1},"col2":{"type":"integer","minimum":1}},"required":["row1","col1","row2","col2"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "confirm",
        summary: "问一个是/否问题，返回布尔。没有人可回答时是错误，不会返回 false 冒充「用户选了否」。",
        params_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"text":{"type":"string"}},"required":["text"]}"#,
        result_schema: FLAG_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "message",
        summary: "显示带按钮组的消息框，返回按钮标签（ok / cancel / yes / no）。按钮组与默认按钮都要在合法集合里。",
        params_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"text":{"type":"string"},"buttons":{"type":"string","enum":["ok","ok_cancel","yes_no","yes_no_cancel"]},"default":{"type":"string"}},"required":["text"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "text_input",
        summary: "显示单行输入框，返回用户输入（可能是空串）。取消是错误（cancelled），不是空串。",
        params_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"prompt":{"type":"string"},"default":{"type":"string"}},"required":["prompt"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "secret",
        summary: "显示密码输入框（掩码），返回输入。CLI 宿主显式拒绝：明文回显比不支持更糟。",
        params_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"prompt":{"type":"string"}},"required":["prompt"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "choose",
        summary: "让用户从若干选项里选一个，返回 1 起序号（与 wait_any 的 index 同一口径）。选项为空是参数错误。",
        params_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"prompt":{"type":"string"},"options":{"type":"array","items":{"type":"string"},"minItems":1}},"required":["options"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "pick_file",
        summary: "打开文件选择框：mode=open/save，返回选中路径（取消返回空串）。用户选中的路径自动追加进本次运行的 fs 授权。",
        params_schema: r#"{"type":"object","properties":{"mode":{"type":"string","enum":["open","save"]},"title":{"type":"string"},"filter":{"type":"string","description":"扩展名过滤，如 *.log"},"default":{"type":"string"}},"required":["mode"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "clipboard_get",
        summary: "读剪贴板文本（只支持 UTF-8 文本；需要 clipboard 授权）。别的程序刚复制的内容也能拿到。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "clipboard_set",
        summary: "把文本写进剪贴板。写不需要授权（不泄露任何东西），失败显式报错。",
        params_schema: r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "selection",
        summary: "取当前会话的选中文本；没有选区时是空串（不是错误，CRT 同语义）。需要 ui 授权与界面宿主。",
        params_schema: NO_PARAMS,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_read",
        summary: "读本地文件（需要 fs 读授权，路径必须落在白名单内）。返回文本内容（非法 UTF-8 按替换字符解码）。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: TEXT_RESULT,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_write",
        summary: "写本地文件（需要 fs 写授权），返回写入字节数。append=true 时追加，缺省覆盖。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1},"data":{"type":"string"},"append":{"type":"boolean","description":"缺省 false（覆盖）"}},"required":["path","data"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_append",
        summary: "追加到本地文件（等价 fs_write 的 append=true），返回追加字节数。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1},"data":{"type":"string"}},"required":["path","data"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_list",
        summary: "列出本地目录，返回 [{name, dir, size}]（按名字排序）。需要 fs 读授权。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: r#"{"type":"object","properties":{"entries":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"dir":{"type":"boolean"},"size":{"type":"integer"}},"required":["name","dir","size"]}}},"required":["entries"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_stat",
        summary: "取本地文件元信息：size / dir / readonly / modified。需要 fs 读授权。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: r#"{"type":"object","properties":{"size":{"type":"integer"},"dir":{"type":"boolean"},"readonly":{"type":"boolean"},"modified":{"type":"integer"}},"required":["size","dir","readonly"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_mkdir",
        summary: "创建本地目录（含父目录）。需要 fs 写授权。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "fs_remove",
        summary: "删除本地文件或空目录。需要 fs 写授权；不存在是错误，不是空成功。",
        params_schema: r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "proc_run",
        summary: "起一个外部进程并等它结束（需要 process 授权），返回 stdout / stderr / code。不经过 shell，参数原样传给程序。",
        params_schema: r#"{"type":"object","properties":{"program":{"type":"string","minLength":1},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_ms":{"type":"integer","minimum":1}},"required":["program"]}"#,
        result_schema: r#"{"type":"object","properties":{"code":{"type":["integer","null"]},"stdout":{"type":"string"},"stderr":{"type":"string"}},"required":["stdout","stderr"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "app_quit",
        summary: "退出应用（需要 ui 授权）：save=true 时先保存布局与会话库。无界面宿主显式报不支持。",
        params_schema: r#"{"type":"object","properties":{"save":{"type":"boolean","description":"缺省 true"}},"required":[]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "window",
        summary: "显示或隐藏主窗口（需要 ui 授权）。无界面宿主显式报不支持。",
        params_schema: r#"{"type":"object","properties":{"visible":{"type":"boolean"}},"required":["visible"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_list",
        summary: "列出远端目录（SFTP），返回 [{name, dir, size}]。会话名省略时用当前绑定；不支持传输的会话显式报错。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: r#"{"type":"object","properties":{"entries":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"dir":{"type":"boolean"},"size":{"type":"integer"}},"required":["name","dir","size"]}}},"required":["entries"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_get",
        summary: "把远端文件下载到本地路径，返回字节数。本地路径要过 fs 写授权。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"remote":{"type":"string","minLength":1},"local":{"type":"string","minLength":1}},"required":["remote","local"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_put",
        summary: "把本地文件上传到远端路径，返回字节数。本地路径要过 fs 读授权。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"local":{"type":"string","minLength":1},"remote":{"type":"string","minLength":1}},"required":["local","remote"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_mkdir",
        summary: "在远端建一层目录（父目录必须已存在，不做 mkdir -p——中途哪一层因权限\n                  失败该停在哪里是策略问题，交给调用方连发几次更清楚）。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_rename",
        summary: "在远端改名或移动一个路径。目标已存在时由服务端决定（多数会覆盖）——\n                  这里不做「目标存在就拒绝」的预检：预检与服务端之间还有竞态，假装原子反而分裂语义。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"from":{"type":"string","minLength":1},"to":{"type":"string","minLength":1}},"required":["from","to"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_remove",
        summary: "删一个远端文件或**空**目录。按属性的结果自三分派，不做递归删除——\n                  一条命令清空整棵树的代价太大，要删树请自己走 sftp_list + sftp_remove。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: OK_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "sftp_stat",
        summary: "取一条远端路径的属性：大小 / 是不是目录 / 是否只读 / 修改时间。\n                  时间取不到时是 null 而不是 0——「不知道」与「1970 年」是两件事。",
        params_schema: r#"{"type":"object","properties":{"session":{"type":"string"},"path":{"type":"string","minLength":1}},"required":["path"]}"#,
        result_schema: r#"{"type":"object","properties":{"size":{"type":"integer"},"dir":{"type":"boolean"},"readonly":{"type":"boolean"},"modified":{"type":"integer"}},"required":["size","dir","readonly"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "profile_list",
        summary: "列出会话库里的全部会话（名字与配置摘要）。无会话库的宿主显式报不支持。",
        params_schema: NO_PARAMS,
        result_schema: r#"{"type":"object","properties":{"sessions":{"type":"array","items":{"type":"object"}}},"required":["sessions"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "profile_get",
        summary: "按名字取一条会话的配置（含协议字段，进程内使用）。",
        params_schema: r#"{"type":"object","properties":{"name":{"type":"string","minLength":1}},"required":["name"]}"#,
        result_schema: r#"{"type":"object","properties":{"config":{"type":"object"}},"required":["config"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "profile_set",
        summary: "按名字覆盖保存一条会话配置（没有就新增）。返回保存后库里的条数。",
        params_schema: r#"{"type":"object","properties":{"name":{"type":"string","minLength":1},"config":{"type":"object"}},"required":["name","config"]}"#,
        result_schema: COUNT_RESULT,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    // ── 应用面（ADR-0027） ────────────────────────────
    //
    // 两条都只对脚本可见（`Exposure::Script`）：唯一的 RPC 宿主 `desk serve`
    // 没有窗口，`app_state` 在那里只能返回 `Unsupported`——给它一个永远失败的
    // 远程方法，比不给更糟（客户端会当它可用）。
    MethodSpec {
        name: "app_state",
        summary: "读一份应用状态快照：面板的可见性与停靠位、主区窗格数、窗口旗标、\
                  偏好的语言，以及两个互相独立的版本号（界面 rev / 偏好 prefs_rev）。\
                  面板恒十条（按 id 取，不必判断长度）；无界面的宿主显式报不支持，\
                  不返回空快照冒充成功。",
        params_schema: NO_PARAMS,
        result_schema: r#"{"type":"object","properties":{"rev":{"type":"integer","minimum":0,"description":"界面状态版本：每次 app_panel_set / app_layout 成功后 +1"},"panels":{"type":"array","description":"恒十条，顺序即 PanelId::ALL","items":{"type":"object","properties":{"id":{"type":"string","enum":["pane_explorer","filer","sidebar","pane_outline","pane_sender","pane_session","pane_shell","pane_transfer","pane_sysmon","pane_script"]},"visible":{"type":"boolean"},"zone":{"type":"string","enum":["left","right","bottom"]},"order":{"type":"integer","minimum":0},"floating":{"type":"boolean"},"rect":{"type":["array","null"],"items":{"type":"integer"}}},"required":["id","visible","zone","order","floating"]}},"layout":{"type":"object","properties":{"leaves":{"type":"integer","minimum":1,"description":"主区窗格数 = Layout::leaf_count()"}},"required":["leaves"]},"window":{"type":"object","properties":{"full_screen":{"type":"boolean"},"always_on_top":{"type":"boolean"},"focus_mode":{"type":"boolean"},"opacity":{"type":"number"}},"required":["full_screen","always_on_top","focus_mode"]},"prefs":{"type":"object","properties":{"language":{"type":"string"}},"required":["language"],"description":"白名单键的落盘值，不是设置页里未保存的编辑"},"prefs_rev":{"type":"integer","minimum":0,"description":"偏好版本：每次 app_prefs_set 成功后 +1；与 rev 相互独立"}},"required":["rev","panels","layout","window","prefs","prefs_rev"]}"#,
        kind: MethodKind::Read,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "app_panel_set",
        summary: "置位一块面板的显示开关（幂等）：设成同值时 changed 为空、rev 不动——\
                  这是它区别于 TogglePanel 的全部理由。只改可见性、不动停靠位、不落盘\
                  （重启恢复出厂态）；未知面板名是错误，不静默无操作。回执带写入之后\
                  的十块面板全景，脚本不必再发一次 app_state 去赌时序。",
        params_schema: r#"{"type":"object","properties":{"panel":{"type":"string","minLength":1,"description":"panel 取 app_state 里 panels[].id 的小写名（filer / pane_session …）；十个之外的名字一律报错（tmux 面板不在停靠表里，同样报错）"},"visible":{"type":"boolean","description":"目标可见性；与现状相同则是空操作"}},"required":["panel","visible"]}"#,
        result_schema: r#"{"type":"object","properties":{"changed":{"type":"array","items":{"type":"string"},"description":"真的变了的字段路径（如 panels.filer）；设成同值时为空数组"},"rev":{"type":"integer","minimum":0,"description":"写入之后的界面状态版本"},"panels":{"type":"array","description":"写入之后的十块面板（形状与 app_state 的 panels 完全一致）","items":{"type":"object","properties":{"id":{"type":"string","enum":["pane_explorer","filer","sidebar","pane_outline","pane_sender","pane_session","pane_shell","pane_transfer","pane_sysmon","pane_script"]},"visible":{"type":"boolean"},"zone":{"type":"string","enum":["left","right","bottom"]},"order":{"type":"integer","minimum":0},"floating":{"type":"boolean"},"rect":{"type":["array","null"],"items":{"type":"integer"}}},"required":["id","visible","zone","order","floating"]}}},"required":["changed","rev","panels"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
    MethodSpec {
        name: "app_layout",
        summary: "改停靠位：reset 回默认布局 / dock 把某块面板停到某带第 index 位 / \
                  redock 收回它的默认带。缺省 index = 追加到该带末尾，显式越界是错误\
                  （夹取只服务拖拽，不服务 API）；必然落盘（layout.json），所以授权\
                  要 app=configure。浮窗定位不在这里：落点来自指针，给脚本只会得到\
                  不可复现的结果。",
        params_schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["reset","dock","redock"],"description":"reset = 全部回默认布局；dock = 停到指定带；redock = 浮窗收回它的默认带"},"panel":{"type":"string","minLength":1,"description":"面板名（取 app_state 的 panels[].id）；dock / redock 必填"},"zone":{"type":"string","enum":["left","right","bottom"],"description":"目标停靠带；dock 必填"},"index":{"type":"integer","minimum":0,"description":"带内位次（0 起）；缺省 = 追加到该带末尾，超过该带成员数是错误"}},"required":["action"]}"#,
        result_schema: r#"{"type":"object","properties":{"changed":{"type":"array","items":{"type":"string"},"description":"真的变了的字段路径（如 panels.filer / layout）"},"rev":{"type":"integer","minimum":0,"description":"写入之后的界面状态版本"},"panels":{"type":"array","description":"写入之后的十块面板（形状与 app_state 的 panels 完全一致）","items":{"type":"object","properties":{"id":{"type":"string","enum":["pane_explorer","filer","sidebar","pane_outline","pane_sender","pane_session","pane_shell","pane_transfer","pane_sysmon","pane_script"]},"visible":{"type":"boolean"},"zone":{"type":"string","enum":["left","right","bottom"]},"order":{"type":"integer","minimum":0},"floating":{"type":"boolean"},"rect":{"type":["array","null"],"items":{"type":"integer"}}},"required":["id","visible","zone","order","floating"]}}},"required":["changed","rev","panels"]}"#,
        kind: MethodKind::Write,
        exposure: Exposure::Script,
    },
];

/// 按名字查方法条目（**不分前端**）。`None` = 这个方法不在能力面上（调用方应报
/// `method_not_found`，不要静默当成「没有参数」）。
pub fn find(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.name == name)
}

/// 远程前端的动词表（`Exposure` 允许 RPC 的那些），顺序同 [`METHODS`]。
pub fn rpc_specs() -> impl Iterator<Item = &'static MethodSpec> {
    METHODS.iter().filter(|spec| spec.exposure.on_rpc())
}

/// 脚本前端的动词表，顺序同 [`METHODS`]。
pub fn script_specs() -> impl Iterator<Item = &'static MethodSpec> {
    METHODS.iter().filter(|spec| spec.exposure.on_script())
}

/// 远程前端的全部方法名。`describe` 的回答、Python 桩、桥接测试都以此为准。
pub fn names() -> Vec<&'static str> {
    rpc_specs().map(|spec| spec.name).collect()
}

/// 脚本前端的全部方法名。`term-script` 的注册集合必须与它**相等**（ADR-0023）。
pub fn script_names() -> Vec<&'static str> {
    script_specs().map(|spec| spec.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_method_has_a_unique_name() {
        let mut seen = HashSet::new();
        for spec in METHODS {
            assert!(
                seen.insert(spec.name),
                "方法名重复：{}（重名会让 dispatch 静默命中其中一个）",
                spec.name
            );
            assert!(!spec.name.is_empty(), "方法名不能为空");
        }
        assert_eq!(names().len(), rpc_specs().count());
    }

    #[test]
    fn every_schema_is_a_valid_json_object() {
        for spec in METHODS {
            for (label, text) in [
                ("params_schema", spec.params_schema),
                ("result_schema", spec.result_schema),
            ] {
                let value: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|e| {
                    panic!("{} 的 {} 不是合法 JSON：{e}（原文：{text}）", spec.name, label)
                });
                assert!(
                    value.is_object(),
                    "{} 的 {} 必须是 JSON 对象（实际：{value}）",
                    spec.name,
                    label
                );
            }
        }
    }

    #[test]
    fn params_schemas_are_objects_with_properties_and_required() {
        // 客户端与 Python 桩都靠 `properties` / `required` 生成签名：缺了就是
        // 生成器退化，必须在表这一层挡住，而不是让下游各自兜底。
        for spec in METHODS {
            let value: serde_json::Value = serde_json::from_str(spec.params_schema).expect("合法 JSON");
            assert!(
                value.get("properties").is_some_and(|p| p.is_object()),
                "{} 的 params_schema 缺 properties 对象",
                spec.name
            );
            let required = value
                .get("required")
                .and_then(|r| r.as_array())
                .unwrap_or_else(|| panic!("{} 的 params_schema 缺 required 数组", spec.name));
            let props = value["properties"].as_object().expect("properties 是对象");
            for name in required {
                let name = name.as_str().expect("required 里必须是字符串");
                assert!(
                    props.contains_key(name),
                    "{} 的 required 提到了 {name}，但 properties 里没有它",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn the_minimum_method_set_is_covered() {
        // 最小能力面：少一个都算没有交付那一刀。
        for expected in [
            "describe",
            "session_list",
            "session_open",
            "session_close",
            "write",
            "keys",
            "wait",
            "screen",
            "state",
            "facets",
        ] {
            assert!(find(expected).is_some(), "能力面缺方法：{expected}");
        }
        // ADR-0023 补齐的流式与日志动词：两个前端都要看得见。
        for expected in [
            "read",
            "read_until",
            "wait_any",
            "wait_idle",
            "log_start",
            "log_stop",
            "log_status",
        ] {
            let spec = find(expected).unwrap_or_else(|| panic!("能力面缺方法：{expected}"));
            assert_eq!(
                spec.exposure,
                Exposure::Both,
                "{expected} 应当对两个前端都可见"
            );
        }
        assert!(find("ssh_connect").is_none(), "协议专有方法不得出现在能力面（§B.14）");
    }

    #[test]
    fn stream_methods_are_told_apart_from_idempotent_reads() {
        // 流式动词消费输出：客户端不该把它当幂等读来重试（ADR-0023 §决策 4）。
        for name in ["read", "read_until", "wait_any", "wait_idle"] {
            assert_eq!(
                find(name).expect("存在").kind,
                MethodKind::Stream,
                "{name} 必须标成 Stream，否则客户端会重试式调用而吃掉数据"
            );
        }
        for name in ["screen", "state", "facets", "log_status"] {
            assert_eq!(
                find(name).expect("存在").kind,
                MethodKind::Read,
                "{name} 是纯读取，不该标成 Stream"
            );
        }
    }

    #[test]
    fn script_only_methods_stay_off_the_remote_surface() {
        // ADR-0023：cursor_pos / terminal_size 之所以只给脚本，是因为远程从
        // screen 的结果字段就能拿到同样的数——单列只是多一次往返。
        // session_current / session_use 是另一种理由：远程前端**参数寻址**
        // （每次调用自带 session），没有「当前绑定」这个概念，给远程一个空操作
        // 不如不给。
        // set_synchronous 是第三种：它改的是**后续调用**的语义（粘性状态），
        // 而远程每次调用自带全部参数，没有地方存这个开关、也没法把它回报给
        // 客户端；notify / ask 是第四种：远程的「用户」在客户端那一侧。
        // app_state / app_panel_set 是第五种：唯一能回答它们的宿主是有窗口的
        // `desk`，而 RPC 侧的宿主 `desk serve` 没有窗口（ADR-0027）。
        for name in [
            "cursor_pos",
            "terminal_size",
            "session_current",
            "session_use",
            "set_synchronous",
            "notify",
            "ask",
            "app_state",
            "app_panel_set",
        ] {
            let spec = find(name).expect("存在");
            assert_eq!(spec.exposure, Exposure::Script, "{name} 应只对脚本可见");
            assert!(!names().contains(&name), "{name} 不该出现在远程动词表里");
            assert!(script_names().contains(&name), "{name} 应当在脚本动词表里");
        }
        // 反过来：会话的增删查是两端都要的（脚本要批量巡检、远程要管会话）。
        for name in ["session_list", "session_open", "session_close"] {
            let spec = find(name).expect("存在");
            assert_eq!(
                spec.exposure,
                Exposure::Both,
                "{name} 少了任何一端都跑不起来：脚本要巡检、远程要管理会话"
            );
        }
    }

    #[test]
    fn session_shaped_results_share_one_descriptor_schema() {
        // 2026-11-14 定稿：所有「返回一个会话」的动词共用同一份描述符，
        // 于是「开完的会话塞回 session_list」不需要换键名。以前 session_open
        // 返回 `{"tab": id, "name": …}`、session_list 的行是 `{"id": …}`，
        // 两个前端各记一套键——正是 ADR-0023 要消灭的那类漂移。
        // ADR-0024 之后描述符里只剩 name / alive / exit：id 不是寻址键。
        assert_eq!(
            find("session_open").expect("存在").result_schema,
            SESSION_DESCRIPTOR_SCHEMA
        );
        assert_eq!(
            find("session_use").expect("存在").result_schema,
            SESSION_DESCRIPTOR_SCHEMA
        );
        let list: serde_json::Value =
            serde_json::from_str(find("session_list").expect("存在").result_schema).expect("合法 JSON");
        let items = &list["properties"]["sessions"]["items"];
        let descriptor: serde_json::Value =
            serde_json::from_str(SESSION_DESCRIPTOR_SCHEMA).expect("合法 JSON");
        // 比结构不比字面：`result_schema` 是 `&'static str`，没法把常量嵌进
        // session_list 的内部（Rust 的静态字符串不能拼接），所以两边是同一份形状
        // 的两段文本，描述文字可以不同。真正的不变量是**键集合与必填项相同**。
        assert_eq!(
            field_names(items),
            field_names(&descriptor),
            "session_list 的每一项必须与 session_open 的结果同形，否则「开完塞回清单」要换键名"
        );
        assert_eq!(items["required"], descriptor["required"]);
    }

    #[test]
    fn app_write_replies_carry_the_same_panels_shape_as_the_snapshot() {
        // `panels` 的 items 在两条结果 schema 里各写了一份（静态字符串没法拼接）。
        // 两份文本会漂，所以比结构：键集合与必填项必须一致——否则「写完接着按
        // app_state 的键读」会在两处之间对不上。
        let state: serde_json::Value =
            serde_json::from_str(find("app_state").expect("存在").result_schema)
                .expect("合法 JSON");
        let state_item = &state["properties"]["panels"]["items"];
        // 两条写类动词各有一份回执 schema（静态字符串没法互相嵌入），都得与
        // `app_state` 的 `panels` 同形——「写完接着按快照的键读」才成立。
        for write_verb in ["app_panel_set", "app_layout"] {
            let delta: serde_json::Value =
                serde_json::from_str(find(write_verb).expect("存在").result_schema)
                    .expect("合法 JSON");
            let delta_item = &delta["properties"]["panels"]["items"];
            assert_eq!(
                field_names(state_item),
                field_names(delta_item),
                "{write_verb} 回执里的 panels 必须与 app_state 的 panels 同形"
            );
            assert_eq!(state_item["required"], delta_item["required"]);
        }
    }

    /// schema 对象里 `properties` 的键，排序后返回（比较形状用）。
    fn field_names(schema: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<String> = schema["properties"]
            .as_object()
            .expect("properties 是对象")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    #[test]
    fn every_kind_is_used_and_serializes_as_snake_case() {
        for kind in [
            MethodKind::Introspect,
            MethodKind::Read,
            MethodKind::Write,
            MethodKind::SessionLifecycle,
            MethodKind::Stream,
        ] {
            assert!(
                METHODS.iter().any(|spec| spec.kind == kind),
                "{kind:?} 类别没有任何方法使用"
            );
        }
        let text = serde_json::to_string(&MethodKind::SessionLifecycle).expect("可序列化");
        assert_eq!(text, "\"session_lifecycle\"");
        let text = serde_json::to_string(&MethodKind::Stream).expect("可序列化");
        assert_eq!(text, "\"stream\"");
        let text = serde_json::to_string(&Exposure::Both).expect("可序列化");
        assert_eq!(text, "\"both\"");
        let text = serde_json::to_string(&MethodSpec {
            name: "x",
            summary: "y",
            params_schema: NO_PARAMS,
            result_schema: NO_PARAMS,
            kind: MethodKind::Read,
            exposure: Exposure::Both,
        })
        .expect("可序列化");
        assert!(text.contains("\"kind\":\"read\""), "MethodSpec 序列化丢了 kind：{text}");
    }

    #[test]
    fn every_exposure_is_used() {
        for exposure in [Exposure::Rpc, Exposure::Script, Exposure::Both] {
            assert!(
                METHODS.iter().any(|spec| spec.exposure == exposure),
                "{exposure:?} 没有任何方法使用——可见性标注退化成了装饰"
            );
        }
    }

    #[test]
    fn the_two_projections_partition_the_table() {
        // 两个前端动词表的并集 == 全表；交集 == Both 那些。
        let all: HashSet<&str> = METHODS.iter().map(|s| s.name).collect();
        let rpc: HashSet<&str> = names().into_iter().collect();
        let script: HashSet<&str> = script_names().into_iter().collect();
        let union: HashSet<&str> = rpc.union(&script).copied().collect();
        assert_eq!(union, all, "有动词两个前端都看不见——它永远不会被任何客户端调用");
        let both: HashSet<&str> = METHODS
            .iter()
            .filter(|s| s.exposure == Exposure::Both)
            .map(|s| s.name)
            .collect();
        let overlap: HashSet<&str> = rpc.intersection(&script).copied().collect();
        assert_eq!(overlap, both, "交集应当恰好是 Both 那一档");
    }

    #[test]
    fn summaries_are_prompt_friendly() {
        // 「prompt 友好」的判据：不空、不指向别处、不是占位符。
        for spec in METHODS {
            let summary = spec.summary;
            assert!(summary.chars().count() >= 20, "{} 的 summary 太短：{summary}", spec.name);
            for banned in ["TODO", "todo", "待补", "见文档", "..."] {
                assert!(
                    !summary.contains(banned),
                    "{} 的 summary 含占位符 {banned}",
                    spec.name
                );
            }
        }
    }
}
