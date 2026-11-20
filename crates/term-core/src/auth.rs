//! OneKey 凭据回退链：一条有序链依次尝试，第一个成功即停（roadmap D2 的裁定内核）。
//!
//! 企业机器的登录方式并不统一：有的只认密码，有的只认 keyboard-interactive
//! （密码页之外还有 OTP / 二次验证），有的只认公钥，有的 agent 里已经装好了票。
//! 单选一个 [`crate::session::AuthMethod`] 一旦选错就登不上，用户只能挨个去试。
//! 本模块把「试哪个、按什么顺序试、失败之后怎么办」收进一个纯状态机：
//!
//! 1. [`AuthChain::build`] 用一个本地能力清单（[`Available`]）把**不可能成功的
//!    方式先剔掉**——不是等它失败，而是根本不放进链里，避免「问了用户一次密码，
//!    其实本地根本没有」这种假交互；
//! 2. 宿主每拿到一次尝试的结果（[`Outcome`]）就调用 [`AuthChain::advance`]，
//!    由它回答「下一步做什么」（[`Step`]）；
//! 3. 全失败时 [`AuthChain::report`] 逐个说明每种方式各发生了什么。
//!
//! 本层不认识文件系统、不读环境变量、不看表：`~/.ssh` 有没有 key、agent 有没有
//! 在跑、密码框弹不弹，全部由宿主探测后以 [`Available`] 传入。因此满足 I3 / B1
//! 意义上的零 IO、零线程、零墙钟。
//!
//! # 为什么是这个顺序
//!
//! 规范顺序为 Password → KeyboardInteractive → Agent → PublicKey → GssapiWithMic，
//! 理由不是「企业经验」，而是每条方式的交互成本与失败代价不同：
//!
//! - **Password 最先**：交互最少、最可解释——一次输入、一个确定的答案，失败原因
//!   只有「密码不对」。放在最前面时，能用密码的人永远只看到一次提示。
//! - **KeyboardInteractive 第二**：它常常承载 OTP / 密码二次验证（服务端逐条提问），
//!   所以「密码被拒」之后它是最可能的下一步；反过来，它自己失败通常意味着这台机器
//!   根本没开这条方式，继续往后走没有损失。
//! - **Agent / PublicKey 第三、第四**：两者都无交互，前提是用户**已经配好了**
//!   （agent 里有票 / 磁盘上有对应私钥）。放到密码前面，会让本来直接输密码就能进的
//!   用户先去碰一个可能没有权限的 key——多一轮无用握手，失败时还给不出人话解释。
//!   OpenSSH 自身的默认偏好也是先试公钥、不行再问密码，但那是「key 已配好」的场景；
//!   这里把密码提前，是因为 OneKey 面对的是**用户不知道自己属于哪种方式**的机器。
//!   Agent 在 PublicKey 之前：agent 是「已经加载好的票」，比磁盘密钥少一次口令询问。
//! - **GssapiWithMic 最后**：企业域环境专用（Kerberos 票），普通环境必然
//!   [`Outcome::Unavailable`]，放在任何位置都不会更差，放最后便于报告里读出
//!   「这台机器不在域里」。
//!
//! `Available` 里出现 `HasPassword` 却把 Password 排到后面，只能靠 `prefer`
//! 显式指定（用户在表单里明确选了它），且 `prefer` 只做**重排**、不做增补。
//!
//! # 与 OpenSSH 的一致性
//!
//! [`AuthChain::preferred_authentications`] 直接由同一条链生成
//! `PreferredAuthentications=` 的值，不维护第二份列表——两份列表迟早会漂移，
//! 而「诊断说试过 publickey、命令行却没带 publickey」是最难查的那类偏差。
//! 链为空时返回 `None` 而不是空串：空值等于让 ssh 用自己的默认顺序，
//! 那不是本层表达过的意图。

/// 一种凭据尝试。
///
/// 注意：变体的声明顺序**不是**尝试顺序，规范顺序见 [`AuthChain::build`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// 服务端 `password` 方式。
    Password,
    /// 服务端 `keyboard-interactive` 方式（密码 / OTP / 二次验证都可能走它）。
    KeyboardInteractive,
    /// 用磁盘上的私钥（`publickey`）。
    PublicKey,
    /// 用 ssh-agent 里已加载的票（线上仍是 `publickey`）。
    Agent,
    /// 企业域的 `gssapi-with-mic`。
    GssapiWithMic,
}

impl Attempt {
    /// 规范顺序：`build` 按它遍历，报告里的「未进链」条目也按它排列。
    const CANONICAL: [Attempt; 5] = [
        Attempt::Password,
        Attempt::KeyboardInteractive,
        Attempt::Agent,
        Attempt::PublicKey,
        Attempt::GssapiWithMic,
    ];

    /// 该方式在线上的名字（OpenSSH `PreferredAuthentications` 用语）。
    ///
    /// `Agent` 没有独立的方式名：agent 只是 `publickey` 的密钥来源，线上一律是
    /// `publickey`。因此 [`AuthChain::preferred_authentications`] 会把两者合并。
    pub fn ssh_name(self) -> &'static str {
        match self {
            Attempt::Password => "password",
            Attempt::KeyboardInteractive => "keyboard-interactive",
            Attempt::PublicKey | Attempt::Agent => "publickey",
            Attempt::GssapiWithMic => "gssapi-with-mic",
        }
    }

    /// 该方式需要本地具备哪种能力（`build` 的过滤依据）。
    pub fn availability(self) -> Available {
        match self {
            Attempt::Password => Available::HasPassword,
            Attempt::KeyboardInteractive => Available::HasKeyboardInteractive,
            Attempt::PublicKey => Available::HasKey,
            Attempt::Agent => Available::HasAgent,
            Attempt::GssapiWithMic => Available::HasGssapi,
        }
    }
}

/// 本地具备的能力（由宿主探测后传入；本层不认识文件系统）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Available {
    /// 有密码可问用户。
    HasPassword,
    /// 磁盘上有可用的私钥。
    HasKey,
    /// ssh-agent 在跑（可能有票，也可能没有——空 agent 只会在尝试时被拒）。
    HasAgent,
    /// 有 Kerberos / GSSAPI 票。
    HasGssapi,
    /// 服务端可能问 interactive 问题，且宿主有办法回答。
    HasKeyboardInteractive,
}

impl Available {
    /// 该能力对应哪一种尝试（报告与过滤共用同一张对照表）。
    pub fn attempt(self) -> Attempt {
        match self {
            Available::HasPassword => Attempt::Password,
            Available::HasKey => Attempt::PublicKey,
            Available::HasAgent => Attempt::Agent,
            Available::HasGssapi => Attempt::GssapiWithMic,
            Available::HasKeyboardInteractive => Attempt::KeyboardInteractive,
        }
    }
}

/// 一次尝试的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 服务端接受，登录成功。
    Accepted,
    /// 该方式被服务端拒绝（继续下一个）。
    Rejected,
    /// 该方式根本不可用（本地没有 key / agent 没跑），不必再问用户。
    Unavailable,
    /// 认证阶段结束（服务端断开 / 超过次数），后续不必再试。
    Fatal,
}

/// 推进的结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// 继续，用这个方式再试。
    Try(Attempt),
    /// 登录成功。
    Done,
    /// 没得试了。
    Exhausted,
    /// 别再试了（`Fatal`）。
    Abort,
}

/// 链的生命周期。进入终止态后不再接受推进（避免重试把游标推回去）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// 还在尝试中。
    Idle,
    /// 已成功（`current()` 停在成功项）。
    Done,
    /// 已中止（`Fatal`）。
    Aborted,
    /// 试完了。
    Exhausted,
}

/// 有序尝试链。构造时就把「不可能成功的方式」剔掉，而不是等它失败。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthChain {
    /// 过滤后的有序链（不含被剔除项）。
    chain: Vec<Attempt>,
    /// 当前游标：只有 `Rejected` / `Unavailable` 推进，`Accepted` 停在成功项。
    cursor: usize,
    /// 生命周期，防止终止态之后再推进。
    phase: Phase,
    /// 诊断日志：先记未进链的 `Unavailable`，再按尝试顺序记真实结果。
    log: Vec<(Attempt, Outcome)>,
}

impl AuthChain {
    /// 用本地实际具备的能力构造一条链。
    ///
    /// 顺序固定为 Password → KeyboardInteractive → Agent → PublicKey →
    /// GssapiWithMic（理由见模块文档），然后按 `available` 过滤、去重：
    /// `available` 里重复的元素不会产生重复尝试。
    ///
    /// `prefer` 是用户在表单里明确选定的方式，**只做重排、不做增补**：若它不在
    /// `available` 里（本地根本没这个能力），忽略即可——硬塞进链会造成
    /// 「问用户要了一次密码，其实本地根本没有」的假交互；它仍会以
    /// `(prefer, Unavailable)` 出现在 [`AuthChain::report`] 里，让诊断说清为什么
    /// 用户点的方式没被用上。
    pub fn build(available: &[Available], prefer: Option<Attempt>) -> Self {
        let mut chain: Vec<Attempt> = Vec::new();
        for attempt in Attempt::CANONICAL {
            // `contains` 顺带完成去重：同一种能力出现多次也只入链一次。
            if available.contains(&attempt.availability()) && !chain.contains(&attempt) {
                chain.push(attempt);
            }
        }
        if let Some(preferred) = prefer {
            if let Some(pos) = chain.iter().position(|&a| a == preferred) {
                let head = chain.remove(pos);
                chain.insert(0, head);
            }
        }
        // 未进链的方式（含「prefer 但本地不具备」）全部记成 Unavailable，
        // 顺序取规范顺序：诊断的稳定性不依赖调用方怎么排列 `available`。
        let mut log = Vec::new();
        for attempt in Attempt::CANONICAL {
            if !chain.contains(&attempt) {
                log.push((attempt, Outcome::Unavailable));
            }
        }
        Self {
            chain,
            cursor: 0,
            phase: Phase::Idle,
            log,
        }
    }

    /// 当前该尝试的方式；链空或已试完时为 `None`。
    ///
    /// `Done` 之后仍指向**成功的那一项**——报告与 `PreferredAuthentications`
    /// 都要能复述「最后用的是哪种方式」。
    pub fn current(&self) -> Option<Attempt> {
        self.chain.get(self.cursor).copied()
    }

    /// 消费一次结果，推进游标，返回「下一步该做什么」。
    ///
    /// - `Accepted` → [`Step::Done`]（游标停在成功项）；
    /// - `Rejected` / `Unavailable` → 下一个，试完则 [`Step::Exhausted`]；
    /// - `Fatal` → [`Step::Abort`]，**不再**返回任何 `Try`；
    /// - 空链时任何输入都返回 [`Step::Exhausted`]；
    /// - 已进入终止态后再调用，原样返回该终止结论（不会重新开始尝试）。
    pub fn advance(&mut self, outcome: Outcome) -> Step {
        match self.phase {
            Phase::Done => return Step::Done,
            Phase::Aborted => return Step::Abort,
            Phase::Exhausted => return Step::Exhausted,
            Phase::Idle => {}
        }
        let Some(attempt) = self.current() else {
            self.phase = Phase::Exhausted;
            return Step::Exhausted;
        };
        match outcome {
            Outcome::Accepted => {
                self.log.push((attempt, Outcome::Accepted));
                self.phase = Phase::Done;
                Step::Done
            }
            Outcome::Fatal => {
                self.log.push((attempt, Outcome::Fatal));
                self.phase = Phase::Aborted;
                Step::Abort
            }
            Outcome::Rejected | Outcome::Unavailable => {
                self.log.push((attempt, outcome));
                self.cursor += 1;
                match self.chain.get(self.cursor) {
                    Some(&next) => Step::Try(next),
                    None => {
                        self.phase = Phase::Exhausted;
                        Step::Exhausted
                    }
                }
            }
        }
    }

    /// 还没试过的部分（含当前项）。
    pub fn remaining(&self) -> &[Attempt] {
        &self.chain[self.cursor..]
    }

    /// 给 OpenSSH 用的 `PreferredAuthentications=` 值（逗号分隔、小写）。
    ///
    /// 与 [`AuthChain::current`] 同源——都从 `chain` + `cursor` 现算，不另存列表，
    /// 因此链一推进返回值立刻同步变化。
    ///
    /// 链为空时返回 `None`：空串会让 ssh 退回自己的默认顺序，等于没表达意图。
    /// 另外会把 `Agent` / `PublicKey` 合并成一个 `publickey`（线上无区别）。
    pub fn preferred_authentications(&self) -> Option<String> {
        let mut names: Vec<&'static str> = Vec::new();
        for attempt in self.remaining() {
            let name = attempt.ssh_name();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            None
        } else {
            Some(names.join(","))
        }
    }

    /// 完全部失败后的诊断：每种方式各发生了什么。
    ///
    /// 顺序稳定：先是没进链的方式（规范顺序，全部 `Unavailable`），再按实际尝试
    /// 顺序记录 `Rejected` / `Unavailable` / `Fatal` / `Accepted`。成功时成功项也会
    /// 留下一条 `Accepted`，便于回答「到底哪种方式通了」。
    pub fn report(&self) -> Vec<(Attempt, Outcome)> {
        self.log.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全集：五种能力都在。
    const ALL: [Available; 5] = [
        Available::HasPassword,
        Available::HasKeyboardInteractive,
        Available::HasKey,
        Available::HasAgent,
        Available::HasGssapi,
    ];

    #[test]
    fn empty_availability_builds_an_empty_chain() {
        let chain = AuthChain::build(&[], None);
        assert_eq!(chain.current(), None);
        assert!(chain.remaining().is_empty());
        assert_eq!(chain.preferred_authentications(), None, "空链不得返回空串");
        // 五种方式都记成 Unavailable，且按规范顺序排列。
        assert_eq!(
            chain.report(),
            vec![
                (Attempt::Password, Outcome::Unavailable),
                (Attempt::KeyboardInteractive, Outcome::Unavailable),
                (Attempt::Agent, Outcome::Unavailable),
                (Attempt::PublicKey, Outcome::Unavailable),
                (Attempt::GssapiWithMic, Outcome::Unavailable),
            ]
        );
    }

    #[test]
    fn empty_chain_advances_to_exhausted_for_every_outcome() {
        for outcome in [
            Outcome::Accepted,
            Outcome::Rejected,
            Outcome::Unavailable,
            Outcome::Fatal,
        ] {
            let mut chain = AuthChain::build(&[], None);
            assert_eq!(chain.advance(outcome.clone()), Step::Exhausted);
            assert_eq!(chain.advance(outcome), Step::Exhausted);
            assert_eq!(chain.current(), None);
        }
    }

    #[test]
    fn single_availability_yields_a_single_attempt() {
        let chain = AuthChain::build(&[Available::HasKey], None);
        assert_eq!(chain.current(), Some(Attempt::PublicKey));
        assert_eq!(chain.remaining(), &[Attempt::PublicKey]);
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("publickey")
        );
    }

    #[test]
    fn full_availability_follows_the_canonical_order() {
        let chain = AuthChain::build(&ALL, None);
        assert_eq!(
            chain.remaining(),
            &[
                Attempt::Password,
                Attempt::KeyboardInteractive,
                Attempt::Agent,
                Attempt::PublicKey,
                Attempt::GssapiWithMic,
            ]
        );
        // Agent 与 PublicKey 线上同为 publickey，合并一次。
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("password,keyboard-interactive,publickey,gssapi-with-mic")
        );
        assert!(chain.report().is_empty(), "全能力时没有东西被剔除");
    }

    #[test]
    fn duplicated_availability_does_not_duplicate_attempts() {
        let chain = AuthChain::build(
            &[
                Available::HasPassword,
                Available::HasPassword,
                Available::HasKey,
                Available::HasKey,
                Available::HasAgent,
            ],
            None,
        );
        assert_eq!(
            chain.remaining(),
            &[Attempt::Password, Attempt::Agent, Attempt::PublicKey]
        );
    }

    #[test]
    fn prefer_moves_an_available_attempt_to_the_front() {
        let chain = AuthChain::build(&ALL, Some(Attempt::PublicKey));
        assert_eq!(chain.current(), Some(Attempt::PublicKey));
        assert_eq!(
            chain.remaining(),
            &[
                Attempt::PublicKey,
                Attempt::Password,
                Attempt::KeyboardInteractive,
                Attempt::Agent,
                Attempt::GssapiWithMic,
            ]
        );
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("publickey,password,keyboard-interactive,gssapi-with-mic")
        );
    }

    #[test]
    fn prefer_without_local_capability_is_ignored_and_reported() {
        let chain = AuthChain::build(&[Available::HasPassword], Some(Attempt::PublicKey));
        // 不许硬塞：链里只有 Password，否则会出现「本地根本没 key 却问用户」。
        assert_eq!(chain.current(), Some(Attempt::Password));
        assert_eq!(chain.remaining(), &[Attempt::Password]);
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("password")
        );
        // 但必须解释「你点的方式为什么没用上」，且只记一条。
        assert_eq!(
            chain.report(),
            vec![
                (Attempt::KeyboardInteractive, Outcome::Unavailable),
                (Attempt::Agent, Outcome::Unavailable),
                (Attempt::PublicKey, Outcome::Unavailable),
                (Attempt::GssapiWithMic, Outcome::Unavailable),
            ]
        );
    }

    #[test]
    fn accepted_stops_on_the_successful_attempt() {
        let mut chain = AuthChain::build(&[Available::HasPassword, Available::HasKey], None);
        assert_eq!(chain.current(), Some(Attempt::Password));
        assert_eq!(chain.advance(Outcome::Accepted), Step::Done);
        assert_eq!(chain.current(), Some(Attempt::Password), "游标停在成功项");
        assert_eq!(
            chain.report(),
            vec![
                (Attempt::KeyboardInteractive, Outcome::Unavailable),
                (Attempt::Agent, Outcome::Unavailable),
                (Attempt::GssapiWithMic, Outcome::Unavailable),
                (Attempt::Password, Outcome::Accepted),
            ]
        );
        // 终止态不再推进。
        assert_eq!(chain.advance(Outcome::Accepted), Step::Done);
    }

    #[test]
    fn rejected_advances_to_the_next_attempt() {
        let mut chain = AuthChain::build(&[Available::HasPassword, Available::HasKey], None);
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::PublicKey));
        assert_eq!(chain.current(), Some(Attempt::PublicKey));
    }

    #[test]
    fn unavailable_advances_and_is_recorded() {
        // Agent 在构造时还在，跑到一半死了：此时结果只能是运行时才知道的 Unavailable。
        let mut chain = AuthChain::build(&[Available::HasAgent, Available::HasKey], None);
        assert_eq!(chain.advance(Outcome::Unavailable), Step::Try(Attempt::PublicKey));
        assert_eq!(
            chain.report(),
            vec![
                (Attempt::Password, Outcome::Unavailable),
                (Attempt::KeyboardInteractive, Outcome::Unavailable),
                (Attempt::GssapiWithMic, Outcome::Unavailable),
                (Attempt::Agent, Outcome::Unavailable),
            ]
        );
    }

    #[test]
    fn consecutive_rejections_end_in_exhausted() {
        let mut chain = AuthChain::build(&ALL, None);
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::KeyboardInteractive));
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::Agent));
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::PublicKey));
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::GssapiWithMic));
        assert_eq!(chain.advance(Outcome::Rejected), Step::Exhausted);
        assert_eq!(chain.current(), None);
        assert!(chain.remaining().is_empty());
        assert_eq!(chain.preferred_authentications(), None);
        assert_eq!(chain.advance(Outcome::Rejected), Step::Exhausted);
    }

    #[test]
    fn fatal_aborts_immediately_and_never_returns_try() {
        let mut chain = AuthChain::build(&ALL, None);
        assert_eq!(chain.advance(Outcome::Fatal), Step::Abort);
        // 后面还有四种方式没试，但认证阶段已结束：不许再返回 Try。
        assert_eq!(chain.advance(Outcome::Rejected), Step::Abort);
        assert_eq!(chain.advance(Outcome::Accepted), Step::Abort);
        assert_eq!(chain.current(), Some(Attempt::Password), "中止时游标不推进");
        assert_eq!(
            chain.report().last(),
            Some(&(Attempt::Password, Outcome::Fatal))
        );
    }

    #[test]
    fn preferred_authentications_tracks_the_cursor() {
        let mut chain = AuthChain::build(&ALL, None);
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("password,keyboard-interactive,publickey,gssapi-with-mic")
        );
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::KeyboardInteractive));
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("keyboard-interactive,publickey,gssapi-with-mic")
        );
        assert_eq!(chain.advance(Outcome::Rejected), Step::Try(Attempt::Agent));
        // Agent 与 PublicKey 在线上同名，只剩 publickey 与 gssapi-with-mic。
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("publickey,gssapi-with-mic")
        );
        assert_eq!(chain.advance(Outcome::Accepted), Step::Done);
        assert_eq!(
            chain.preferred_authentications().as_deref(),
            Some("publickey,gssapi-with-mic"),
            "Done 后仍指向成功项所在的后缀"
        );
    }

    #[test]
    fn report_order_is_stable_regardless_of_availability_order() {
        let forward = AuthChain::build(&ALL, None).report();
        let mut reversed = ALL;
        reversed.reverse();
        let backward = AuthChain::build(&reversed, None).report();
        assert_eq!(forward, backward, "available 的排列顺序不影响诊断顺序");
    }
}
