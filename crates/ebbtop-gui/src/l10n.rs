//! Offline UI localization for EbbTop.
//!
//! Three locales live in this file: English (the default and the per-string
//! fallback), Mainland Simplified Chinese (`zh-Hans-CN`) and Taiwan
//! Traditional Chinese (`zh-Hant-TW`). There are no locale files, no network
//! and no translation service: the strings are compiled in, and one macro
//! invocation below is the single source of truth for the key set, so a key
//! that is missing a locale is a compile error rather than a silent English
//! fallback.
//!
//! Two bundled Noto Sans CJK regional faces (SIL OFL 1.1, pinned and
//! hash-recorded in `assets/fonts/PROVENANCE.md`) are compiled in with
//! `include_bytes!` and appended *behind* the existing Latin faces, so English
//! metrics are unchanged and any CJK text — UI strings or raw host/process
//! data — has real glyphs offline.
//!
//! What is *not* here: canonical text from `ebbtop-core` (error tags, log
//! lines, the support summary), config keys/values, and raw host data. Those
//! stay English/verbatim by design (see `docs/l10n-scope.md`).

use egui::text::{FontData, FontFamily};
use std::sync::Arc;

/// Bundled Simplified Chinese face (`Noto Sans SC` Regular 2.004).
pub const FONT_ZH_HANS: &[u8] = include_bytes!("../assets/fonts/NotoSansSC-Regular.otf");
/// Bundled Taiwan Traditional Chinese face (`Noto Sans TC` Regular 2.004).
pub const FONT_ZH_HANT: &[u8] = include_bytes!("../assets/fonts/NotoSansTC-Regular.otf");

const FONT_ZH_HANS_NAME: &str = "noto-sans-sc";
const FONT_ZH_HANT_NAME: &str = "noto-sans-tc";

/// The exact checked-in OFL text for the bundled faces, compiled into the
/// executable with `include_str!` so the license and copyright are readable
/// offline from the shipped binary (Settings > Display). The file is the
/// upstream license for the pinned font commit (see `assets/fonts/PROVENANCE.md`);
/// `l10n`'s notice test compares this constant to the file on disk, so the
/// displayed text cannot drift from the checked-in bytes.
pub const FONT_LICENSE_TEXT: &str = include_str!("../assets/fonts/LICENSE-OFL.txt");

// ---------------------------------------------------------------- locales ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    /// The default, and the fallback for any value this build does not know.
    #[default]
    English,
    /// Mainland Simplified Chinese.
    ZhHansCn,
    /// Taiwan Traditional Chinese.
    ZhHantTw,
}

impl Locale {
    pub const ALL: [Locale; 3] = [Locale::English, Locale::ZhHansCn, Locale::ZhHantTw];

    /// Canonical value stored in `config.json`'s `language` field.
    pub fn id(self) -> &'static str {
        match self {
            Locale::English => "en",
            Locale::ZhHansCn => "zh-Hans-CN",
            Locale::ZhHantTw => "zh-Hant-TW",
        }
    }

    /// The name shown in the Settings language picker, in its own language.
    /// Never translated: a user looking for their own language must be able to
    /// read the option in that language.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::English => "English",
            Locale::ZhHansCn => "简体中文",
            Locale::ZhHantTw => "繁體中文（台灣）",
        }
    }

    /// Resolve a stored config value. Exact canonical ids only: a hand-edited
    /// value, a future locale, a wrong case or a blank string resolves to
    /// English instead of panicking or guessing. Hong Kong (`zh-Hant-HK`) is
    /// deliberately not a locale of this build.
    pub fn from_config(raw: &str) -> Self {
        match raw.trim() {
            "zh-Hans-CN" => Locale::ZhHansCn,
            "zh-Hant-TW" => Locale::ZhHantTw,
            _ => Locale::English,
        }
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

// ------------------------------------------------------------------- keys ---

/// One localized string: the English source and its two translations. Every
/// key carries all three strings, so "untranslated" is never implicit.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub en: &'static str,
    pub sc: &'static str,
    pub tc: &'static str,
}

/// The key set and the three string tables, defined in one place: the enum,
/// `Key::ALL` and `entry()` are generated together, so a key cannot exist
/// without its strings and a missing locale is a compile error.
macro_rules! strings {
    ($( $variant:ident => ($en:expr, $sc:expr, $tc:expr) ),* $(,)?) => {
        /// Every user-facing string of the localized slice.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Key {
            $( $variant ),*
        }

        impl Key {
            /// Every key, for parity, coverage and placeholder tests.
            #[cfg_attr(not(test), allow(dead_code))]
            pub const ALL: &'static [Key] = &[ $( Key::$variant ),* ];
        }

        fn entry(key: Key) -> Entry {
            match key {
                $( Key::$variant => Entry { en: $en, sc: $sc, tc: $tc } ),*
            }
        }
    };
}

strings! {
    // ---- Onboarding wizard ------------------------------------------------
    OnbTitle => ("Welcome to EbbTop", "欢迎使用 EbbTop", "歡迎使用 EbbTop"),
    OnbIntro => (
        "Add your first host. The token is stored in the OS credential store.",
        "添加你的第一台主机。令牌保存在操作系统的凭据存储中。",
        "新增你的第一台主機。權杖儲存在作業系統的認證存放區中。"
    ),
    OnbManualHeader => ("Or enter the address yourself", "或者自己输入地址", "或者自行輸入位址"),
    OnbPortHint => (
        "Agent port 45976; token file ~/services/ebbtop-agent/ebbtop.token on the agent host.",
        "代理端口 45976；令牌文件位于代理主机上的 ~/services/ebbtop-agent/ebbtop.token。",
        "代理程式連接埠 45976；權杖檔案位於代理主機上的 ~/services/ebbtop-agent/ebbtop.token。"
    ),
    OnbFetchHint => (
        "Fetch it with: ssh user@host cat services/ebbtop-agent/ebbtop.token (| pbcopy / | Set-Clipboard)",
        "获取方式：ssh user@host cat services/ebbtop-agent/ebbtop.token（| pbcopy / | Set-Clipboard）",
        "取得方式：ssh user@host cat services/ebbtop-agent/ebbtop.token（| pbcopy / | Set-Clipboard）"
    ),
    OnbBareHostTip => (
        "Tip: a bare hostname works — ws://, :45976 and /ws are added.",
        "提示：直接输入主机名即可 — 会自动补上 ws://、:45976 和 /ws。",
        "提示：直接輸入主機名稱即可 — 會自動補上 ws://、:45976 和 /ws。"
    ),
    LabelName => ("Name", "名称", "名稱"),
    LabelUrl => ("URL", "URL", "URL"),
    LabelToken => ("Token", "令牌", "權杖"),
    HintServerName => ("myserver", "myserver", "myserver"),
    HintUrlLong => (
        "myserver or ws://host:45976/ws",
        "myserver 或 ws://host:45976/ws",
        "myserver 或 ws://host:45976/ws"
    ),
    HintPasteToken => ("paste the agent token", "粘贴代理令牌", "貼上代理程式權杖"),
    AccessAgentToken => ("Agent token", "代理令牌", "代理程式權杖"),
    BtnTest => ("Test", "测试", "測試"),
    BtnSaveConnect => ("Save and connect", "保存并连接", "儲存並連線"),
    BtnUsePairingCode => ("Use a pairing code", "使用配对码", "使用配對碼"),
    HoverUsePairingCode => (
        "probe the address above and exchange a 6-digit code for a token",
        "探测上面的地址，用 6 位配对码换取令牌",
        "探測上方的位址，用 6 位數配對碼換取權杖"
    ),
    BtnSkip => ("Skip", "跳过", "略過"),
    NoteSaveUnlock => (
        "Save unlocks after a green Test for these exact values.",
        "对当前值测试通过后，保存才会解锁。",
        "對目前的值測試通過後，儲存才會解鎖。"
    ),
    NoteFieldsChanged => (
        "fields changed since the last green Test — Test again to unlock Save",
        "字段在上次测试通过后已更改 — 请重新测试以解锁保存",
        "欄位在上次測試通過後已變更 — 請重新測試以解鎖儲存"
    ),
    Testing => ("testing…", "正在测试…", "正在測試…"),
    NoteTypedTokenOverride => (
        "using the token you typed instead of the paired credential",
        "使用你输入的令牌，而不是配对得到的凭据",
        "使用你輸入的權杖，而不是配對取得的認證"
    ),
    NotePairedCredential => (
        "credential: issued by pairing and stored in the OS credential store (not shown)",
        "凭据：由配对签发并保存在操作系统的凭据存储中（不显示）",
        "認證：由配對簽發並儲存在作業系統的認證存放區中（不顯示）"
    ),
    WillConnectTo => ("will connect to {}", "将连接到 {}", "將連線到 {}"),
    ConnectedTo => ("connected: {}", "已连接：{}", "已連線：{}"),
    LogLine => ("log: {}", "日志：{}", "記錄檔：{}"),
    BtnOpenLogs => ("Open logs", "打开日志", "開啟記錄檔"),
    BtnCopySupportSummary => ("Copy support summary", "复制支持摘要", "複製支援摘要"),
    NoteNoCredential => (
        "no credential yet: pair with a 6-digit code from `ebbtop-agent --pair` on the host, or paste the agent token (`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`)",
        "还没有凭据：用主机上 `ebbtop-agent --pair` 生成的 6 位配对码配对，或粘贴代理令牌（`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`）",
        "還沒有認證：用主機上 `ebbtop-agent --pair` 產生的 6 位數配對碼配對，或貼上代理程式權杖（`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`）"
    ),
    NoteTestFirst => (
        "Test the connection first — Save unlocks after a green Test for these exact values",
        "请先测试连接 — 对当前值测试通过后保存才会解锁",
        "請先測試連線 — 對目前的值測試通過後儲存才會解鎖"
    ),
    TestUnavailable => (
        "test unavailable (no runtime)",
        "无法测试（没有运行时）",
        "無法測試（沒有執行階段）"
    ),
    PairedSavedOnboarding => (
        "paired with {}: token saved in the OS credential store. Test to unlock Save.",
        "已与 {} 配对：令牌已保存在操作系统的凭据存储中。测试后即可保存。",
        "已與 {} 配對：權杖已儲存在作業系統的認證存放區中。測試後即可儲存。"
    ),
    PairedSessionOnlyOnboarding => (
        "paired with {}: the OS credential store is unavailable, so the token is kept for this session only. Test to unlock Save.",
        "已与 {} 配对：操作系统的凭据存储不可用，令牌仅在本次会话中保留。测试后即可保存。",
        "已與 {} 配對：作業系統的認證存放區無法使用，權杖僅在本次工作階段中保留。測試後即可儲存。"
    ),
    PairedSavedSettings => (
        "paired {}: token saved in the OS credential store — Test it in the list above, then set it active.",
        "已与 {} 配对：令牌已保存在操作系统的凭据存储中 — 请在上面的列表中测试，然后设为当前来源。",
        "已與 {} 配對：權杖已儲存在作業系統的認證存放區中 — 請在上方的清單中測試，然後設為目前來源。"
    ),
    PairedSessionOnlySettings => (
        "paired {}: the OS credential store is unavailable, so the token is kept for this session only.",
        "已与 {} 配对：操作系统的凭据存储不可用，令牌仅在本次会话中保留。",
        "已與 {} 配對：作業系統的認證存放區無法使用，權杖僅在本次工作階段中保留。"
    ),
    PairedNotAdded => (
        "paired, but the source was not added: {}",
        "配对成功，但未能添加来源：{}",
        "配對成功，但未能新增來源：{}"
    ),

    // ---- Settings window --------------------------------------------------
    SettingsTitle => ("Settings", "设置", "設定"),
    TabSources => ("Sources", "来源", "來源"),
    TabDisplay => ("Display", "显示", "顯示"),
    TabConnection => ("Connection", "连接", "連線"),
    TabUpdates => ("Updates", "更新", "更新"),
    NoSourcesAddBelow => (
        "no sources yet; add one below",
        "还没有来源；请在下面添加",
        "還沒有來源；請在下方新增"
    ),
    HoverSetActiveSource => ("set active source", "设为当前来源", "設為目前來源"),
    BtnRemove => ("Remove", "移除", "移除"),
    HoverConnectOnce => ("connect once to {}", "连接一次 {}", "連線一次 {}"),
    AccessNewSourceName => ("New source name", "新来源名称", "新來源名稱"),
    AccessNewSourceUrl => ("New source URL", "新来源 URL", "新來源 URL"),
    HintUrlShort => ("ws://host:port/ws", "ws://host:port/ws", "ws://host:port/ws"),
    BtnAdd => ("Add", "添加", "新增"),
    AddedSource => ("added {}", "已添加 {}", "已新增 {}"),
    PairNewHost => ("Pair a new host", "配对新主机", "配對新主機"),
    PairRunHint => (
        "Run `ebbtop-agent --pair` on the host for a 6-digit code (about 5 minutes, one use), then pick it below or type its address. No token copying needed.",
        "在主机上运行 `ebbtop-agent --pair` 获取 6 位配对码（约 5 分钟有效，仅可使用一次），然后在下面选择它或输入地址。无需复制令牌。",
        "在主機上執行 `ebbtop-agent --pair` 取得 6 位數配對碼（約 5 分鐘有效，僅可使用一次），然後在下方選擇它或輸入位址。無需複製權杖。"
    ),
    LabelAddress => ("Address", "地址", "位址"),
    HintHostOrWs => ("host or ws://host:45976/ws", "主机或 ws://host:45976/ws", "主機或 ws://host:45976/ws"),
    AccessAddressToPair => ("Address to pair with", "用于配对的地址", "用於配對的位址"),
    BtnPairWithCode => ("Pair with code", "用配对码配对", "用配對碼配對"),
    HoverPairWithCode => (
        "pair with the address you typed, using a 6-digit code",
        "用你输入的地址和 6 位配对码进行配对",
        "用你輸入的位址和 6 位數配對碼進行配對"
    ),
    BadUrl => ("bad URL: {}", "地址无效：{}", "位址無效：{}"),
    NameEmpty => ("name is empty", "名称为空", "名稱為空"),
    SourceExists => (
        "source \"{}\" already exists",
        "来源“{}”已存在",
        "來源「{}」已存在"
    ),
    PanelsHeader => ("Panels", "面板", "面板"),
    PanelCpu => ("CPU", "CPU", "CPU"),
    PanelGpu => ("GPU", "GPU", "GPU"),
    PanelMem => ("Mem", "内存", "記憶體"),
    PanelNet => ("Net", "网络", "網路"),
    PanelDisk => ("Disk", "磁盘", "磁碟"),
    TopHeight => ("Top height", "顶部高度", "頂部高度"),
    Dense => ("Dense", "紧凑", "精簡"),
    HoverDense => ("tighter spacing", "更紧凑的间距", "更精簡的間距"),
    LogScale => ("log scale", "对数刻度", "對數刻度"),
    LanguageLabel => ("Language", "语言", "語言"),
    LanguageHover => (
        "app language; applies immediately and is saved for the next start",
        "应用语言；立即生效，并在下次启动时沿用",
        "應用程式語言；立即生效，並在下次啟動時沿用"
    ),
    SourceLine => ("source: {}", "来源：{}", "來源：{}"),
    NoSourcesAddUnder => (
        "no sources yet; add one under Sources",
        "还没有来源；请在“来源”中添加",
        "還沒有來源；請在「來源」中新增"
    ),
    LabelAgentUrl => ("Agent URL", "代理 URL", "代理程式 URL"),
    HintKeepSavedToken => (
        "leave empty to keep the saved token",
        "留空以保留已保存的令牌",
        "留空以保留已儲存的權杖"
    ),
    LabelStored => ("Stored", "存储位置", "儲存位置"),
    StoredEnv => ("from environment variable", "来自环境变量", "來自環境變數"),
    StoredKeyring => ("in OS credential store", "在操作系统的凭据存储中", "在作業系統的認證存放區中"),
    StoredMissing => ("no token saved", "未保存令牌", "未儲存權杖"),
    StoredError => ("credential store error: {}", "凭据存储错误：{}", "認證存放區錯誤：{}"),
    ConfigPath => ("config: {}", "配置：{}", "設定：{}"),
    BtnSaveReconnect => ("Save and reconnect", "保存并重新连接", "儲存並重新連線"),
    ConfigNotSaved => ("config not saved: {}", "配置未保存：{}", "設定未儲存：{}"),
    TokenSessionOnly => (
        "token used for this session only, store failed: {}",
        "令牌仅在本次会话中使用，存储失败：{}",
        "權杖僅在本次工作階段中使用，儲存失敗：{}"
    ),
    UpdatesNeverChecked => ("never checked", "从未检查", "從未檢查"),
    UpdatesChecking => ("checking…", "正在检查…", "正在檢查…"),
    UpdatesAvailable => ("{} available", "{} 可用", "{} 可用"),
    UpdatesUpToDate => ("up to date ({})", "已是最新（{}）", "已是最新（{}）"),
    UpdatesFailed => ("update failed: {}", "更新失败：{}", "更新失敗：{}"),
    UpdatesApproved => ("{} approved", "{} 已批准", "{} 已核准"),
    UpdatesDownloading => ("downloading {} ({} MB)", "正在下载 {}（{} MB）", "正在下載 {}（{} MB）"),
    UpdatesReadyRestart => ("{} ready to restart", "{} 可以重启安装", "{} 可以重新啟動安裝"),
    BtnCheckUpdates => ("Check for updates", "检查更新", "檢查更新"),
    BtnDownloadUpdate => ("Download update {}", "下载更新 {}", "下載更新 {}"),

    // ---- Process table and detail pane -----------------------------------
    WaitingForData => ("waiting for data", "正在等待数据", "正在等待資料"),
    FilterHint => (
        "filter name / user / pid (/)",
        "筛选 名称 / 用户 / PID（/）",
        "篩選 名稱 / 使用者 / PID（/）"
    ),
    AccessFilterProcesses => ("Filter processes", "筛选进程", "篩選處理程序"),
    CountOf => ("{} of {}", "{} / {} 项", "{} / {} 項"),
    WorkloadChip => ("workload: {}", "工作负载：{}", "工作負載：{}"),
    AtTime => ("at {}", "时间 {}", "時間 {}"),
    ColPid => ("PID", "PID", "PID"),
    ColName => ("Name", "名称", "名稱"),
    ColUser => ("User", "用户", "使用者"),
    ColState => ("S", "S", "S"),
    ColCpuPct => ("CPU%", "CPU%", "CPU%"),
    ColMemPct => ("MEM%", "MEM%", "MEM%"),
    ColMem => ("MEM", "内存", "記憶體"),
    ColWorkload => ("Workload", "工作负载", "工作負載"),
    ThisHost => ("this host", "本机", "本機"),
    KillRootOwned => (
        "root-owned processes cannot be killed",
        "属于 root 的进程无法终止",
        "屬於 root 的處理程序無法終止"
    ),
    KillNoStartTimes => (
        "agent does not report process start times",
        "代理不报告进程启动时间",
        "代理程式未回報處理程序啟動時間"
    ),
    KillHintOnHost => ("terminate this process on {}", "在 {} 上终止此进程", "在 {} 上終止此處理程序"),
    KillHover => ("{} (pid {})", "{}（PID {}）", "{}（PID {}）"),
    BtnKill => ("Kill", "终止", "終止"),
    BtnCopy => ("Copy", "复制", "複製"),
    HoverCopyCommand => ("copy full command", "复制完整命令行", "複製完整命令列"),
    FactsThreads => ("threads {}", "线程 {}", "執行緒 {}"),
    FactsFds => ("fds {}", "文件描述符 {}", "檔案描述元 {}"),
    FactsPpid => ("ppid {}", "父进程 {}", "父處理程序 {}"),
    FactsCpu => ("cpu {}", "CPU 时间 {}", "CPU 時間 {}"),
    FactsVmem => ("vmem {}", "虚拟内存 {}", "虛擬記憶體 {}"),
    FactsShared => ("shared {}", "共享内存 {}", "共用記憶體 {}"),
    FactsReadWrite => ("r {} · w {}", "读 {} · 写 {}", "讀 {} · 寫 {}"),
    FactsRead => ("read {}", "读取 {}", "讀取 {}"),
    FactsWritten => ("written {}", "写入 {}", "寫入 {}"),
    FactsStarted => ("started {}", "启动于 {}", "啟動於 {}"),
    FactsCwd => ("cwd {}", "工作目录 {}", "工作目錄 {}"),
    ChildrenHeader => ("children:", "子进程：", "子處理程序："),
    MoreCount => ("+{} more", "另有 {} 个", "另有 {} 個"),
    ThreadHeader => ("TID Name S CPU", "TID 名称 S CPU", "TID 名稱 S CPU"),
    NoDetail => ("no detail", "无详细信息", "無詳細資訊"),

    // ---- Connection state word -------------------------------------------
    ConnLive => ("LIVE", "实时", "即時"),
    ConnStale => ("STALE", "已过期", "已過期"),
    ConnConnecting => ("CONNECTING", "连接中", "連線中"),
    ConnDown => ("DOWN", "已断开", "已中斷"),

    // ---- Classified connection failures (English must equal core) --------
    ErrAuthRefusedHeadline => ("Closed right after the handshake", "握手后立即关闭", "握手後立即關閉"),
    ErrAuthRefusedNextAction => (
        "EbbTop infers this from one observation: the endpoint accepted the WebSocket handshake and closed at once. The EbbTop agent closes that way to refuse a token. Re-copy the token on the agent host (`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`), paste it again and Test. The close is not proof: another WebSocket service at that host and port can close the same way, so also confirm the host and port point at the EbbTop agent (default port 45976). If both are right and the token is definitely the current one, the agent may have been reinstalled with a new token: run `systemctl --user status ebbtop-agent` on the agent host.",
        "EbbTop 只凭一次观察作出这一判断：端点接受了 WebSocket 握手后立即关闭。EbbTop 代理拒绝令牌时就是这样关闭的。请在代理主机上重新复制令牌（`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`），再次粘贴并测试。关闭本身不是证据：该主机和端口上的其他 WebSocket 服务也可能这样关闭，因此还要确认主机和端口指向的是 EbbTop 代理（默认端口 45976）。如果两者都正确且令牌确定是最新的，代理可能已用新令牌重新安装：在代理主机上运行 `systemctl --user status ebbtop-agent`。",
        "EbbTop 只憑一次觀察作出這項判斷：端點接受了 WebSocket 握手後立即關閉。EbbTop 代理程式拒絕權杖時就是這樣關閉的。請在代理主機上重新複製權杖（`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`），再次貼上並測試。關閉本身不是證據：該主機和連接埠上的其他 WebSocket 服務也可能這樣關閉，因此還要確認主機和連接埠指向的是 EbbTop 代理程式（預設連接埠 45976）。如果兩者都正確且權杖確定是最新的，代理程式可能已用新權杖重新安裝：在代理主機上執行 `systemctl --user status ebbtop-agent`。"
    ),
    ErrUnreachableHeadline => ("No route to the agent", "无法连接到代理", "無法連線到代理程式"),
    ErrUnreachableNextAction => (
        "Nothing is reachable at that address. Check the IP or hostname, that this computer is on the same private network or VPN as the agent, the port (default 45976), and that the agent is running (`systemctl --user is-active ebbtop-agent` on the agent host).",
        "该地址无法访问。请检查 IP 或主机名、这台计算机是否与代理在同一私有网络或 VPN 中、端口（默认 45976），以及代理是否正在运行（在代理主机上运行 `systemctl --user is-active ebbtop-agent`）。",
        "該位址無法連線。請檢查 IP 或主機名稱、這台電腦是否與代理程式在同一個私有網路或 VPN 中、連接埠（預設 45976），以及代理程式是否正在執行（在代理主機上執行 `systemctl --user is-active ebbtop-agent`）。"
    ),
    ErrTimeoutHeadline => ("No answer in time", "未及时收到响应", "未及時收到回應"),
    ErrTimeoutNextAction => (
        "The address did not answer in time. A firewall rule, a VPN that is down and a stopped agent all look like this. Confirm the IP, the private network or VPN, and the port; check `systemctl --user is-active ebbtop-agent` on the agent host; then Test again.",
        "该地址未及时响应。防火墙规则、VPN 断开和代理停止运行都会表现为这样。请确认 IP、私有网络或 VPN 以及端口；在代理主机上检查 `systemctl --user is-active ebbtop-agent`；然后重新测试。",
        "該位址未及時回應。防火牆規則、VPN 中斷和代理程式停止執行都會表現成這樣。請確認 IP、私有網路或 VPN 以及連接埠；在代理主機上檢查 `systemctl --user is-active ebbtop-agent`；然後重新測試。"
    ),
    ErrNotAgentHeadline => ("Not the EbbTop agent", "不是 EbbTop 代理", "不是 EbbTop 代理程式"),
    ErrNotAgentNextAction => (
        "Something answered, but it is not the EbbTop agent. Check the port (default 45976) and the path (`/ws`), and that nothing else is listening there. A bare hostname gets `ws://`, `:45976` and `/ws` added for you.",
        "有响应，但不是 EbbTop 代理。请检查端口（默认 45976）和路径（`/ws`），并确认没有其他服务在该处监听。只输入主机名时会自动补上 `ws://`、`:45976` 和 `/ws`。",
        "有回應，但不是 EbbTop 代理程式。請檢查連接埠（預設 45976）和路徑（`/ws`），並確認沒有其他服務在該處監聽。只輸入主機名稱時會自動補上 `ws://`、`:45976` 和 `/ws`。"
    ),
    ErrAmbiguousHeadline => ("Connection failed", "连接失败", "連線失敗"),
    ErrAmbiguousNextAction => (
        "The connection failed without a clear cause. Check the IP, the private network or VPN, the port, and the agent status (`systemctl --user is-active ebbtop-agent` on the agent host), then Test again.",
        "连接失败，原因不明。请检查 IP、私有网络或 VPN、端口以及代理状态（在代理主机上运行 `systemctl --user is-active ebbtop-agent`），然后重新测试。",
        "連線失敗，原因不明。請檢查 IP、私有網路或 VPN、連接埠以及代理程式狀態（在代理主機上執行 `systemctl --user is-active ebbtop-agent`），然後重新測試。"
    ),

    // ---- Canonical strings that must stay readable in every locale -------
    DetailPrefix => ("detail: {}", "详情：{}", "詳細資訊：{}"),
    TokenUnusual => (
        "token looks unusual: the agent token is 64 hex characters from ebbtop.token",
        "令牌格式异常：代理令牌是 ebbtop.token 中的 64 位十六进制字符",
        "權杖格式異常：代理程式權杖是 ebbtop.token 中的 64 位十六進位字元"
    ),
    // ---- Kill confirmation and outcome -----------------------------------
    KillTitle => ("Kill process", "终止进程", "終止處理程序"),
    KillConfirmBody => (
        r#"Send SIGTERM to "{}" (pid {})? {} will terminate this process. This cannot be undone."#,
        "向“{}”（PID {}）发送 SIGTERM？{} 将终止此进程，且无法撤销。",
        "向「{}」（PID {}）傳送 SIGTERM？{} 將終止此處理程序，且無法復原。"
    ),
    BtnSendTerm => ("Send TERM", "发送 TERM", "傳送 TERM"),
    BtnSendInt => ("Send INT", "发送 INT", "傳送 INT"),
    BtnCancel => ("Cancel", "取消", "取消"),
    KillSent => ("kill sent: pid {} {}", "已发送终止信号：PID {} {}", "已傳送終止訊號：PID {} {}"),
    KillUnavailable => (
        "kill unavailable: no client running",
        "无法终止：没有正在运行的客户端",
        "無法終止：沒有正在執行的用戶端"
    ),
    HostFallback => ("The server", "服务器", "伺服器"),

    // ---- Status-bar chrome ------------------------------------------------
    NoSource => ("no source", "无来源", "沒有來源"),
    Age => ("age {}", "已过 {}", "已過 {}"),
    AgeHover => (
        "time since a new sample arrived, local clock{}\n{}",
        "距上次收到新采样的时间（本机时钟）{}\n{}",
        "距上次收到新取樣的時間（本機時鐘）{}\n{}"
    ),
    DiagSampleTick => (
        "\nsample {} · tick p95 {}{}",
        "\n采样 {} · 周期 p95 {}{}",
        "\n取樣 {} · 週期 p95 {}{}"
    ),
    DiagSample => ("\nsample {}", "\n采样 {}", "\n取樣 {}"),
    DiagReconnects => (" · reconnects {}", " · 重连 {} 次", " · 重新連線 {} 次"),
    DownRetry => ("{}; retry {}", "{}；{} 后重试", "{}；{} 後重試"),
    AttemptN => ("attempt {}", "第 {} 次尝试", "第 {} 次嘗試"),
    KillOk => ("kill {} {} sent", "已发送终止信号：PID {} {}", "已傳送終止訊號：PID {} {}"),
    KillRefused => ("kill {} refused: {}", "PID {} 终止被拒绝：{}", "PID {} 終止被拒絕：{}"),
    KillErrorFallback => ("error", "错误", "錯誤"),
    UpdateReadyBar => ("update {} ready", "更新 {} 已就绪", "更新 {} 已就緒"),
    BtnUpdate => ("Update", "更新", "更新"),
    UpdateCheckFailed => ("update check failed", "更新检查失败", "更新檢查失敗"),
    BtnSettings => ("Settings", "设置", "設定"),
    ComboSource => ("source", "来源", "來源"),

    // ---- Top panels: CPU / GPU / memory / network / disk -----------------
    // Panel headers. English keeps the base build's lower-case titles
    // (`v cpu`), so no existing assertion or screenshot caption moves.
    PanelTitleCpu => ("cpu", "CPU", "CPU"),
    PanelTitleGpu => ("gpu", "GPU", "GPU"),
    PanelTitleMem => ("mem", "内存", "記憶體"),
    PanelTitleNet => ("net", "网络", "網路"),
    PanelTitleDisk => ("disk", "磁盘", "磁碟"),
    PanelTitleProc => ("proc", "进程", "處理程序"),
    PanelTitleProcDetail => ("proc-detail", "进程详情", "處理程序詳細資料"),
    PanelTitleWorkloads => ("workloads", "工作负载", "工作負載"),
    Cores => ("{} cores", "{} 个核心", "{} 個核心"),
    LoadAvg => ("load {} {} {}", "负载 {} {} {}", "負載 {} {} {}"),
    Uptime => ("up {}", "运行 {}", "執行 {}"),
    MeterUsed => ("Used", "已用", "已用"),
    MeterSwap => ("Swap", "交换", "置換"),
    MeterMeasuring => ("measuring", "正在测量", "正在測量"),
    LabelInterface => ("interface", "网络接口", "網路介面"),
    Peak => ("peak {}", "峰值 {}", "峰值 {}"),
    MeterLoadGpu => ("Load", "负载", "負載"),
    MeterVram => ("VRAM", "显存", "視訊記憶體"),
    ComboDisk => ("disk", "磁盘", "磁碟"),
    DiskAuto => ("auto", "自动", "自動"),

    // ---- Workload roll-up -------------------------------------------------
    ColPids => ("PIDs", "进程数", "處理程序數"),
    HoverClearWorkload => (
        "clear the workload filter",
        "清除工作负载筛选",
        "清除工作負載篩選"
    ),
    AccessClearWorkload => ("Clear workload filter", "清除工作负载筛选", "清除工作負載篩選"),

    // ---- Update dialog (updater builds only) -----------------------------
    UpdateAvailTitle => ("Update available", "有可用更新", "有可用更新"),
    UpdateDownloadingTitle => ("Downloading update", "正在下载更新", "正在下載更新"),
    UpdateRestartTitle => ("Restart to update", "重启以更新", "重新啟動以更新"),
    UpdateFailedTitle => ("Update failed", "更新失败", "更新失敗"),
    UpdateAvailBody => (
        "EbbTop {} is available (you have {}).",
        "EbbTop {} 已发布（当前版本 {}）。",
        "EbbTop {} 已發布（目前版本 {}）。"
    ),
    BtnDownload => ("Download update", "下载更新", "下載更新"),
    BtnLater => ("Later", "稍后", "稍後"),
    UpdateDownloadingBody => (
        "Downloading EbbTop {}… {} MB so far.",
        "正在下载 EbbTop {}… 已完成 {} MB。",
        "正在下載 EbbTop {}… 已完成 {} MB。"
    ),
    UpdateReadyBody => (
        "EbbTop {} is downloaded and verified.",
        "EbbTop {} 已下载并校验通过。",
        "EbbTop {} 已下載並驗證通過。"
    ),
    BtnRestartUpdate => ("Restart to update", "重启以更新", "重新啟動以更新"),
    UpdateFailedBody => ("Update failed: {}", "更新失败：{}", "更新失敗：{}"),
    BtnCheckAgain => ("Check again", "重新检查", "重新檢查"),

    // ---- Discovery scanner and pairing-code prompt -----------------------
    ScanCancel => ("Cancel scan", "取消扫描", "取消掃描"),
    ScanningSegment => ("scanning this network segment…", "正在扫描此网段…", "正在掃描此網段…"),
    BtnFindAgents => ("Find agents on this network", "查找此网络中的代理", "尋找此網路中的代理程式"),
    HoverFindAgents => (
        "UDP broadcast probe, 1.4 s, cancellable",
        "UDP 广播探测，1.4 秒，可取消",
        "UDP 廣播探測，1.4 秒，可取消"
    ),
    ScanScopeNote => (
        "Scans this network segment only: broadcast does not cross routers, VLANs or a VPN/tailnet. Reachable over a tailnet or another subnet? Enter the address yourself below.",
        "仅扫描此网段：广播不会穿过路由器、VLAN 或 VPN/tailnet。若目标在 tailnet 或其他子网中可达，请在下方自己输入地址。",
        "僅掃描此網段：廣播不會穿過路由器、VLAN 或 VPN/tailnet。若目標在 tailnet 或其他子網段中可達，請在下方自行輸入位址。"
    ),
    BtnUseThisHost => ("Use this host", "使用此主机", "使用此主機"),
    HoverUseThisHost => (
        "pick this agent and pair with a code",
        "选择此代理并用配对码配对",
        "選擇此代理程式並用配對碼配對"
    ),
    NoAgentAnswered => (
        "No agent answered. Check that the host runs the agent with discovery enabled (EBBTOP_AGENT_DISCOVERY is on by default), that its firewall allows UDP 45978, or enter the address yourself below.",
        "没有代理响应。请检查主机是否启用了发现功能来运行代理（EBBTOP_AGENT_DISCOVERY 默认开启）、其防火墙是否允许 UDP 45978，或在下方自己输入地址。",
        "沒有代理程式回應。請確認主機是否啟用探索功能執行代理程式（EBBTOP_AGENT_DISCOVERY 預設開啟）、其防火牆是否允許 UDP 45978，或在下方自行輸入位址。"
    ),
    ScanStartFailed => ("could not start the scan: {}", "无法开始扫描：{}", "無法開始掃描：{}"),
    PairWith => ("Pair with {}", "与 {} 配对", "與 {} 配對"),
    PairCodeHint => (
        "{} — run `ebbtop-agent --pair` on that host for a 6-digit code (about 5 minutes, one use).",
        "{} — 在该主机上运行 `ebbtop-agent --pair` 获取 6 位配对码（约 5 分钟有效，仅可使用一次）。",
        "{} — 在該主機上執行 `ebbtop-agent --pair` 取得 6 位數配對碼（約 5 分鐘有效，僅可使用一次）。"
    ),
    LabelPairingCode => ("Pairing code", "配对码", "配對碼"),
    HintSixDigits => ("6 digits", "6 位数字", "6 位數字"),
    BtnPair => ("Pair", "配对", "配對"),
    HoverPairExchange => (
        "exchange the code for a read-only token on that host",
        "用配对码换取该主机上的只读令牌",
        "用配對碼換取該主機上的唯讀權杖"
    ),
    PairBusy => ("asking the host for a token…", "正在向主机申请令牌…", "正在向主機申請權杖…"),
    PairAddrBad => ("cannot pair with this address: {}", "无法与此地址配对：{}", "無法與此位址配對：{}"),
    PairStartFailed => ("could not start the exchange: {}", "无法开始配对交换：{}", "無法開始配對交換：{}"),

    // ---- Classified pairing failures (English must equal core) -----------
    PairNoPendingCodeHeadline => (
        "No pairing code is waiting on that host",
        "该主机上没有等待中的配对码",
        "該主機上沒有等待中的配對碼"
    ),
    PairNoPendingCodeNextAction => (
        "Run `ebbtop-agent --pair` on the agent host to mint a code, then enter it here within about five minutes. A code is single use: if you already paired with it, this is expected and no action is needed beyond pairing again only if you need another client.",
        "请在代理主机上运行 `ebbtop-agent --pair` 生成配对码，然后在约五分钟内在此输入。配对码只能使用一次：如果你已经用它配对过，这是正常现象，除非还需要为另一个客户端配对，否则无需再操作。",
        "請在代理主機上執行 `ebbtop-agent --pair` 產生配對碼，然後在約五分鐘內在此輸入。配對碼只能使用一次：如果你已經用它配對過，這是正常現象，除非還需要為另一個用戶端配對，否則無需再操作。"
    ),
    PairInvalidCodeHeadline => ("Pairing code not accepted", "配对码未被接受", "配對碼未被接受"),
    PairInvalidCodeNextAction => (
        "Re-type the six digits exactly as the agent host printed them. Five wrong guesses cancel the code: if you have already tried several, run `ebbtop-agent --pair` on the host for a new one.",
        "请按代理主机打印的六位数字原样重新输入。猜错五次后配对码会作废：如果已经试过几次，请在主机上运行 `ebbtop-agent --pair` 获取新的配对码。",
        "請按代理主機列印的六位數字原樣重新輸入。猜錯五次後配對碼會失效：如果已經試過幾次，請在主機上執行 `ebbtop-agent --pair` 取得新的配對碼。"
    ),
    PairExpiredCodeHeadline => ("Pairing code expired", "配对码已过期", "配對碼已過期"),
    PairExpiredCodeNextAction => (
        "Codes last about five minutes. Run `ebbtop-agent --pair` on the agent host for a fresh code and enter it straight away.",
        "配对码有效期约五分钟。请在代理主机上运行 `ebbtop-agent --pair` 获取新配对码，并立即输入。",
        "配對碼有效期約五分鐘。請在代理主機上執行 `ebbtop-agent --pair` 取得新配對碼，並立即輸入。"
    ),
    PairLockedOutHeadline => ("Too many wrong codes", "配对码错误次数过多", "配對碼錯誤次數過多"),
    PairLockedOutNextAction => (
        "The host cancelled that code after five wrong guesses (the deliberate brute-force limit). Run `ebbtop-agent --pair` on the agent host for a new code.",
        "猜错五次后主机会作废该配对码（这是有意设置的防暴力破解限制）。请在代理主机上运行 `ebbtop-agent --pair` 获取新配对码。",
        "猜錯五次後主機會作廢該配對碼（這是刻意設定的防暴力破解限制）。請在代理主機上執行 `ebbtop-agent --pair` 取得新配對碼。"
    ),
    PairBadRequestHeadline => (
        "The host did not understand the pairing request",
        "主机无法理解该配对请求",
        "主機無法理解該配對請求"
    ),
    PairBadRequestNextAction => (
        "The host answered, but refused the request shape. Confirm the address is the EbbTop agent (default port 45976) and that its version supports pairing (0.2.9 or newer); an older agent has no `/pair`, so paste its token instead.",
        "主机有响应，但拒绝了请求格式。请确认地址是 EbbTop 代理（默认端口 45976），且其版本支持配对（0.2.9 或更新）；较旧的代理没有 `/pair`，请改为粘贴其令牌。",
        "主機有回應，但拒絕了請求格式。請確認位址是 EbbTop 代理程式（預設連接埠 45976），且其版本支援配對（0.2.9 或更新）；較舊的代理程式沒有 `/pair`，請改為貼上其權杖。"
    ),
    PairUnreachableHeadline => ("No route to the host", "无法连接到主机", "無法連線到主機"),
    PairUnreachableNextAction => (
        "Nothing is reachable at that address. Check the host, that this computer is on the same private network or VPN as the agent, the port (default 45976), and that the agent is running (`systemctl --user is-active ebbtop-agent` on the host).",
        "该地址无法访问。请检查主机、这台计算机是否与代理在同一私有网络或 VPN 中、端口（默认 45976），以及代理是否正在运行（在主机上运行 `systemctl --user is-active ebbtop-agent`）。",
        "該位址無法連線。請檢查主機、這台電腦是否與代理程式在同一個私有網路或 VPN 中、連接埠（預設 45976），以及代理程式是否正在執行（在主機上執行 `systemctl --user is-active ebbtop-agent`）。"
    ),
    PairTimeoutHeadline => ("The host did not answer in time", "主机未及时响应", "主機未及時回應"),
    PairTimeoutNextAction => (
        "The host did not answer in time. A firewall, a VPN that is down and a stopped agent all look like this. Confirm the host and port, check `systemctl --user is-active ebbtop-agent` on the host, then pair again.",
        "主机未及时响应。防火墙、VPN 断开和代理停止运行都会表现为这样。请确认主机和端口，在主机上检查 `systemctl --user is-active ebbtop-agent`，然后重新配对。",
        "主機未及時回應。防火牆、VPN 中斷和代理程式停止執行都會表現成這樣。請確認主機和連接埠，在主機上檢查 `systemctl --user is-active ebbtop-agent`，然後重新配對。"
    ),
    PairNotAgentHeadline => ("Not the EbbTop agent", "不是 EbbTop 代理", "不是 EbbTop 代理程式"),
    PairNotAgentNextAction => (
        "Something answered, but not the EbbTop agent. Check the port (default 45976) and that nothing else is listening there; a bare hostname gets `:45976` added.",
        "有响应，但不是 EbbTop 代理。请检查端口（默认 45976），并确认没有其他服务在该处监听；只输入主机名时会自动补上 `:45976`。",
        "有回應，但不是 EbbTop 代理程式。請檢查連接埠（預設 45976），並確認沒有其他服務在該處監聽；只輸入主機名稱時會自動補上 `:45976`。"
    ),
    PairAmbiguousHeadline => ("Pairing failed", "配对失败", "配對失敗"),
    PairAmbiguousNextAction => (
        "The exchange failed without a clear cause. Check the host, the port (default 45976) and the agent status (`systemctl --user is-active ebbtop-agent` on the host), then try again — or paste the agent token instead.",
        "配对交换失败，原因不明。请检查主机、端口（默认 45976）和代理状态（在主机上运行 `systemctl --user is-active ebbtop-agent`），然后重试 — 或改为粘贴代理令牌。",
        "配對交換失敗，原因不明。請檢查主機、連接埠（預設 45976）和代理程式狀態（在主機上執行 `systemctl --user is-active ebbtop-agent`），然後重試 — 或改為貼上代理程式權杖。"
    ),

    // ---- GUI-facing support / log actions and their notes ----------------
    SupportCopiedSaved => (
        "support summary copied (saved to {})",
        "支持摘要已复制（已保存到 {}）",
        "支援摘要已複製（已儲存至 {}）"
    ),
    SupportCopied => (
        "support summary copied to the clipboard",
        "支持摘要已复制到剪贴板",
        "支援摘要已複製到剪貼板"
    ),
    LogsNoDataDir => (
        "no data directory for the log",
        "日志没有可用的数据目录",
        "記錄檔沒有可用的資料目錄"
    ),
    LogsRunFailed => ("could not run {}: {}", "无法运行 {}：{}", "無法執行 {}：{}"),

    // ---- Bundled font license notice (Settings > Display) ----------------
    NoticeTitle => ("Fonts and license", "字体与许可证", "字型與授權"),
    NoticeHeading => (
        "Bundled fonts and their license",
        "内置字体及其许可证",
        "內建字型及其授權"
    ),
    NoticeIntro => (
        "EbbTop bundles two regional Chinese fonts so Chinese interface text and Chinese host and process names render offline. Both are unmodified upstream files under the SIL Open Font License 1.1.",
        "EbbTop 内置两款中文区域字体，使中文界面文本以及中文主机名和进程名都能离线正常显示。两者均为未修改的上游文件，采用 SIL 开放字体许可证 1.1。",
        "EbbTop 內建兩款中文區域字型，讓中文介面文字以及中文主機名稱和處理程序名稱都能離線正常顯示。兩者皆為未修改的上游檔案，採用 SIL 開放字型授權 1.1。"
    ),
    NoticeFontSc => (
        "Noto Sans SC (Simplified Chinese), version 2.004 — © 2014-2021 Adobe (http://www.adobe.com/).",
        "Noto Sans SC（简体中文）2.004 版 — © 2014-2021 Adobe（http://www.adobe.com/）。",
        "Noto Sans SC（簡體中文）2.004 版 — © 2014-2021 Adobe（http://www.adobe.com/）。"
    ),
    NoticeFontTc => (
        "Noto Sans TC (Traditional Chinese), version 2.004 — © 2014-2021 Adobe (http://www.adobe.com/).",
        "Noto Sans TC（繁体中文·台湾）2.004 版 — © 2014-2021 Adobe（http://www.adobe.com/）。",
        "Noto Sans TC（繁體中文·台灣）2.004 版 — © 2014-2021 Adobe（http://www.adobe.com/）。"
    ),
    NoticeLicenseLabel => (
        "SIL Open Font License 1.1 (full text)",
        "SIL 开放字体许可证 1.1（全文）",
        "SIL 開放字型授權 1.1（全文）"
    ),
}

/// The localized text for `key` in `locale`. All three locales are complete at
/// compile time, so this never falls back per string; the English default only
/// applies when the *locale* is unknown (see [`Locale::from_config`]).
pub fn text(locale: Locale, key: Key) -> &'static str {
    let e = entry(key);
    match locale {
        Locale::English => e.en,
        Locale::ZhHansCn => e.sc,
        Locale::ZhHantTw => e.tc,
    }
}

/// `text` with `{}` placeholders filled left to right. Panics on an argument
/// count that does not match the template — a programming error the
/// placeholder-parity test rules out for every key × locale.
pub fn fmt(locale: Locale, key: Key, args: &[&str]) -> String {
    let template = text(locale, key);
    let parts: Vec<&str> = template.split("{}").collect();
    assert_eq!(
        parts.len() - 1,
        args.len(),
        "placeholder/argument mismatch for {key:?} in {locale}"
    );
    let mut out = String::with_capacity(template.len() + 16 * args.len());
    for (i, part) in parts.iter().enumerate() {
        out.push_str(part);
        if let Some(a) = args.get(i) {
            out.push_str(a);
        }
    }
    out
}

/// Headline and next action for a classified failure, keyed by the stable
/// `ProbeFailure` variant — never by matching English prose.
pub fn failure_keys(kind: ebbtop_core::client::ProbeFailure) -> (Key, Key) {
    use ebbtop_core::client::ProbeFailure as P;
    match kind {
        P::AuthRefused => (Key::ErrAuthRefusedHeadline, Key::ErrAuthRefusedNextAction),
        P::Unreachable => (Key::ErrUnreachableHeadline, Key::ErrUnreachableNextAction),
        P::Timeout => (Key::ErrTimeoutHeadline, Key::ErrTimeoutNextAction),
        P::NotAgent => (Key::ErrNotAgentHeadline, Key::ErrNotAgentNextAction),
        P::Ambiguous => (Key::ErrAmbiguousHeadline, Key::ErrAmbiguousNextAction),
    }
}

/// Localized headline for a classified failure.
pub fn failure_headline(locale: Locale, d: &ebbtop_core::client::Diagnosis) -> &'static str {
    text(locale, failure_keys(d.kind).0)
}

/// Localized next action for a classified failure.
pub fn failure_next_action(locale: Locale, d: &ebbtop_core::client::Diagnosis) -> &'static str {
    text(locale, failure_keys(d.kind).1)
}

/// Headline and next action for a classified *pairing* failure, keyed by the
/// stable `PairFailure` variant — the pairing flow's equivalent of
/// [`failure_keys`], so a new failure class cannot silently skip translation.
pub fn pair_failure_keys(kind: ebbtop_core::pairing::PairFailure) -> (Key, Key) {
    use ebbtop_core::pairing::PairFailure as P;
    match kind {
        P::NoPendingCode => (Key::PairNoPendingCodeHeadline, Key::PairNoPendingCodeNextAction),
        P::InvalidCode => (Key::PairInvalidCodeHeadline, Key::PairInvalidCodeNextAction),
        P::ExpiredCode => (Key::PairExpiredCodeHeadline, Key::PairExpiredCodeNextAction),
        P::LockedOut => (Key::PairLockedOutHeadline, Key::PairLockedOutNextAction),
        P::BadRequest => (Key::PairBadRequestHeadline, Key::PairBadRequestNextAction),
        P::Unreachable => (Key::PairUnreachableHeadline, Key::PairUnreachableNextAction),
        P::Timeout => (Key::PairTimeoutHeadline, Key::PairTimeoutNextAction),
        P::NotAgent => (Key::PairNotAgentHeadline, Key::PairNotAgentNextAction),
        P::Ambiguous => (Key::PairAmbiguousHeadline, Key::PairAmbiguousNextAction),
    }
}

/// Localized headline for a classified pairing failure.
pub fn pair_failure_headline(
    locale: Locale,
    e: &ebbtop_core::pairing::PairError,
) -> &'static str {
    text(locale, pair_failure_keys(e.kind).0)
}

/// Localized next action for a classified pairing failure.
pub fn pair_failure_next_action(
    locale: Locale,
    e: &ebbtop_core::pairing::PairError,
) -> &'static str {
    text(locale, pair_failure_keys(e.kind).1)
}

// ------------------------------------------------------------------ fonts ---

/// Font definitions for `locale`: the shipped set (`epaint_default_fonts`:
/// Hack, Ubuntu-Light, NotoEmoji, emoji-icon-font) untouched and first, with
/// the two bundled CJK faces appended as fallbacks for both families. The
/// locale only decides which regional face is tried first; both are present in
/// every locale, so a Traditional-only codepoint inside a process name still
/// renders in the Simplified locale and vice versa. Raw host/process text is
/// never translated, so this is what keeps it from turning into tofu.
pub fn font_definitions(locale: Locale) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(FONT_ZH_HANS_NAME.to_owned(), Arc::new(FontData::from_static(FONT_ZH_HANS)));
    fonts
        .font_data
        .insert(FONT_ZH_HANT_NAME.to_owned(), Arc::new(FontData::from_static(FONT_ZH_HANT)));

    let order: [&str; 4] = match locale {
        Locale::ZhHantTw => {
            [FONT_ZH_HANT_NAME, FONT_ZH_HANS_NAME, "Ubuntu-Light", "Hack"]
        }
        _ => [FONT_ZH_HANS_NAME, FONT_ZH_HANT_NAME, "Ubuntu-Light", "Hack"],
    };
    // Appended, never inserted at the front: the Latin faces keep their glyphs
    // and metrics, so English text is byte- and pixel-identical to the base.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        list.push(order[0].to_owned());
        list.push(order[1].to_owned());
    }
    fonts
}

/// Install the fonts for `locale` on `ctx`. Cheap to call again on a locale
/// switch; egui rebuilds its glyph atlas and keeps widget state (ids are
/// positional, never text-derived).
pub fn install_fonts(ctx: &egui::Context, locale: Locale) {
    ctx.set_fonts(font_definitions(locale));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_ids_round_trip() {
        for loc in Locale::ALL {
            assert_eq!(Locale::from_config(loc.id()), loc, "{loc:?} id must round-trip");
        }
        assert_eq!(Locale::default(), Locale::English);
        assert_eq!(Locale::from_config(""), Locale::English);
        assert_eq!(Locale::from_config("zh-Hans"), Locale::English);
        assert_eq!(Locale::from_config("zh-Hant-HK"), Locale::English);
        assert_eq!(Locale::from_config("ZH-HANS-CN"), Locale::English);
        assert_eq!(Locale::from_config(" zh-Hans-CN "), Locale::ZhHansCn);
        assert_eq!(Locale::from_config("fr"), Locale::English);
    }

    #[test]
    fn fmt_fills_placeholders_in_order() {
        assert_eq!(fmt(Locale::English, Key::CountOf, &["3", "7"]), "3 of 7");
        assert_eq!(fmt(Locale::ZhHansCn, Key::CountOf, &["3", "7"]), "3 / 7 项");
        assert_eq!(
            fmt(Locale::ZhHantTw, Key::ConnectedTo, &["k-server"]),
            "已連線：k-server"
        );
    }

    #[test]
    fn english_error_text_matches_core() {
        use ebbtop_core::client::ProbeFailure as P;
        for kind in [P::AuthRefused, P::Unreachable, P::Timeout, P::NotAgent, P::Ambiguous] {
            let (h, n) = failure_keys(kind);
            assert_eq!(text(Locale::English, h), kind.headline(), "{kind:?} headline drift");
            assert_eq!(text(Locale::English, n), kind.next_action(), "{kind:?} next_action drift");
        }
    }
}
