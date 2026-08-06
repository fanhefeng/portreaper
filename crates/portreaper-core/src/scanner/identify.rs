//! identify_app 的跨平台共享部件：路径切分、项目名/脚本名提取。
//! 平台特定的「路径阶梯」（/Applications vs Program Files）在 macos.rs / windows.rs。

/// 脚本运行时（不带 .exe 后缀的小写名；Windows 侧比较前先 strip_exe）。
pub(crate) const SCRIPT_RUNTIMES: &[&str] = &[
    "node", "python", "python3", "ruby", "java", "javaw", "bun", "deno", "php", "perl",
];

/// 双分隔符 basename：/usr/bin/zsh → zsh，C:\Windows\cmd.exe → cmd.exe
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// 编译期 dev 产物的路径片段（cargo target、`go run` 临时目录）—— 分隔符与大小写
/// 归一后双平台共用一份。这些是「路径结构」而非项目名关键字（前导 `/` 已锚定，
/// 不会被 ~/code/my-go-build-tools/ 这类用户目录名误触），必须先于路径豁免拿到
/// dev-script 身份，否则孤儿 `go run` / cargo 产物会被标准路径前缀整体放行。
/// 评审发现：原先 macos.rs / windows.rs 各维护一份、仅分隔符不同，工具链新增需
/// 双改，且 Windows 无人工 QA —— 漏改即静默回归。新增片段只改这一处。
pub(crate) fn is_dev_build_artifact(path: &str) -> bool {
    let norm = path.replace('\\', "/").to_lowercase();
    norm.contains("/target/debug/")
        || norm.contains("/target/release/")
        || norm.contains("/go-build")
}

/// 开发工具自带 / 下载的运行时目录 —— 这些位置里的「应用」是项目或工具链的
/// 开发期 runtime，不是用户安装的应用：electron / electron-vite 把 Electron.app
/// 放在 node_modules/electron/dist，Playwright / Puppeteer 把 Chromium.app 下载到
/// ~/Library/Caches/ms-playwright（Windows：%LOCALAPPDATA%\ms-playwright），形态与
/// /Applications 里的真应用一模一样。不摘出来就会吃 installed-app 硬豁免，
/// 孤儿化的 dev runtime 永远检测不到（真实漏报：孤儿 Electron 主进程）。
/// 用户安装的应用绝不会住在这些目录里，故此信号零误伤。
/// 分隔符与大小写归一后双平台共用一份（新增片段只改这一处）。
pub(crate) fn is_dev_tool_runtime_path(path: &str) -> bool {
    let norm = path.replace('\\', "/").to_lowercase();
    [
        "/node_modules/",
        "/ms-playwright/", // Playwright 下载的浏览器（macOS Caches / Windows LocalAppData）
        "/.cache/puppeteer/", // Puppeteer 默认下载目录
        "/.local-chromium/", // 旧版 Puppeteer 布局
        "/.cache/selenium/", // Selenium Manager 下载的浏览器 / driver
        "/webdriver-manager/", // webdriver-manager 下载目录
    ]
    .iter()
    .any(|p| norm.contains(p))
}

/// 一次性自动化浏览器实例的**命令行**特征（跨平台：Chromium / Firefox 的这些
/// 开关在 macOS 与 Windows 上逐字相同，故实现放共享层，两平台不各写一份）。
///
/// 存在意义见 docs/KNOWN-GAPS.md Gap 1：headless Chrome 由自动化框架
/// （Playwright / Puppeteer / devtools MCP …）拉起，宿主可执行文件恰好住在
/// /Applications，会被 installed-app 硬豁免整体放行 —— 但它真正的身份是
/// 「一次性自动化会话」，和 `python app.py` 的身份是脚本、而非解释器安装位置
/// 完全同构。命中者由 identify_app 归入 automation-instance 类别，从路径豁免里摘出。
///
/// 判据：`--headless` 是**必要条件**，再叠加一条「自动化会话」证据。
/// 为什么必要条件不可省（实测反例，KNOWN-GAPS A2）：只靠「调试端口 + 临时
/// profile」会直接命中所有**有头**的自动化实例 —— 那正是用户此刻正在用的
/// 浏览器窗口，误杀会打断一个活跃会话。
///
/// ⚠️ 命中本函数**不等于**判定为残留：它只是把行摘出路径豁免，仍必须有孤儿
/// 信号才会被标记；且调试端口上有活跃客户端连接时被 DebuggerAttached 一票否决。
pub(crate) fn is_automation_instance(full_command: &str) -> bool {
    let mut headless = false;
    let mut session_evidence = false;
    let mut tokens = full_command.split_whitespace().peekable();
    while let Some(tok) = tokens.next() {
        let lower = tok.to_lowercase();
        // Chromium `--headless` / `--headless=new`；Firefox 单横线 `-headless`
        if lower == "--headless" || lower == "-headless" || lower.starts_with("--headless=") {
            headless = true;
            continue;
        }
        // 调试端口 / 调试管道 / webdriver 标记：不带值的开关直接采信
        if lower.starts_with("--remote-debugging-port")
            || lower == "--remote-debugging-pipe"
            || lower == "--enable-automation"
            || lower == "-marionette"          // Firefox 的 webdriver 通道
            || lower == "--marionette"
        {
            session_evidence = true;
            continue;
        }
        // 临时目录里的 profile：一次性会话的强特征（真实用户 profile 在家目录下）
        let temp_profile_value = ["--user-data-dir", "-profile", "--profile"]
            .iter()
            .find_map(|flag| {
                lower
                    .strip_prefix(flag)
                    .and_then(|rest| match rest.strip_prefix('=') {
                        Some(v) => Some(v.to_string()),
                        // 分离值形（`-profile /tmp/x`）：值是下一个 token
                        None if rest.is_empty() => tokens.peek().map(|v| v.to_lowercase()),
                        None => None,
                    })
            });
        if temp_profile_value.is_some_and(|v| is_temp_dir_path(&v)) {
            session_evidence = true;
        }
    }
    headless && session_evidence
}

/// 路径值（**命令行参数值**，不是 exe 路径）是否落在系统临时目录下。
/// 刻意独立于 `is_standard_install_path`：macOS 的 /private/var/folders/ 在那边是
/// **豁免项**（为 App Translocation 让路），这里恰恰是「一次性」的证据 ——
/// 两者语义相反，绝不能共用同一个函数（KNOWN-GAPS Gap 1 明示的坑）。
fn is_temp_dir_path(value: &str) -> bool {
    let norm = value.replace('\\', "/").to_lowercase();
    norm.starts_with("/tmp/")
        || norm.starts_with("/private/tmp/")
        || norm.starts_with("/var/folders/")
        || norm.starts_with("/private/var/folders/")
        || norm.contains("/appdata/local/temp/")
        || norm.contains("/windows/temp/")
}

/// 自动化实例的标签：「Chrome · headless」—— 主名取 .app 名（macOS bundle）或
/// exe 基名。前端 splitLabel 按 " · " 拆成主/副两行渲染。
pub(crate) fn automation_label(exe_path: &str, short_command: &str) -> String {
    let name = exe_path
        .find(".app/")
        .and_then(|idx| {
            let before = &exe_path[..idx];
            before.rfind('/').map(|slash| &before[slash + 1..])
        })
        .map(|app| app.to_string())
        .unwrap_or_else(|| strip_exe(basename(exe_path)).to_string());
    let name = if name.is_empty() {
        strip_exe(short_command).to_string()
    } else {
        name
    };
    format!("{name} · headless")
}

/// 去掉 Windows 可执行后缀：node.exe → node（大小写不敏感）
pub(crate) fn strip_exe(name: &str) -> &str {
    // is_char_boundary 守卫:name 末尾若是非 ASCII 多字节字符(如中文进程名),
    // n-4 可能落在该字符内部,裸切片 `name[n-4..]` 会 panic —— 先确认是字符
    // 边界再比对。同时省去原 to_lowercase() 的整串堆分配(`.exe` 全 ASCII,
    // eq_ignore_ascii_case 足够,且不会把非 ASCII 大写映射进来)。
    let n = name.len();
    if n >= 4 && name.is_char_boundary(n - 4) && name[n - 4..].eq_ignore_ascii_case(".exe") {
        &name[..n - 4]
    } else {
        name
    }
}

/// 「分离值」本身可能是脚本/配置路径、但都不是进程入口的带值选项 —— 必须跳过其值
///（评审实锤：`node --import ./reg.js server.mjs` 曾把 OpenTelemetry/tsx 的注入
/// 脚本当入口，标签、重复配对、brew 豁免三处全部用错身份）。粘连形
/// `--import=./x.js` 整个 token 以 `-` 开头，由通用选项跳过规则天然覆盖。
/// 与 extract_module_arg 处理 `-W ignore` 分离值是同一类问题的镜像。
///
/// `--config` 也在列且必须保留：`node --config app.config.js server.mjs` 里
/// app.config.js 是配置不是入口，不跳过它就会被当成入口脚本（评审复核：移除
/// `--config` 才是真正的回归）。它与预加载类的区别仅在「非运行时程序」上有意义
/// （如 `vite --config vite.config.ts` 取不到入口、回退到 short_command/项目名）——
/// 但 vite 不是 SCRIPT_RUNTIMES，标签不走此路径，重复检测也有 full_command / cwd
/// 兜底，故无可观察损失。新增带值选项时一并补回归用例（见下方测试）。
const SCRIPT_VALUE_FLAGS: &[&str] = &[
    "-r",
    "--require",
    "--import",
    "--loader",
    "--experimental-loader",
    "--preload",
    "--config",
    // java 的 classpath 家族：值是 jar/目录但不是入口 —— `java -cp bootstrap.jar
    // com.Main` 曾把 bootstrap.jar 当入口，进而给 gradle daemon / tomcat 这类
    // 有意长驻的 -cp 启动进程贴上 dev-script、孤儿化即 Confirmed（评审实锤）。
    // 裸 `-p`（--module-path 短形）刻意不收：与 node 的 -p（eval 模式）语义冲突。
    "-cp",
    "-classpath",
    "--class-path",
    "--module-path",
];

/// eval / stdin 模式（`python -c`、`node -e`、`-`）—— ps 剥掉引号后代码体的
/// token 不可信，里面恰好以脚本扩展名结尾的词不是入口脚本。extract_script_arg
/// 与 extract_module_arg 共用此熔断，避免两函数对同一类输入语义漂移（评审发现：
/// `python -c "import sys" app.py` 曾把 app.py 误当入口）。
fn is_eval_mode_flag(tok: &str) -> bool {
    matches!(tok, "-c" | "-e" | "-")
}

/// 从完整命令行中找真正的「入口脚本」（.js/.ts/.py/...）：
/// 跳过所有选项 token 及已知带值选项的分离值，在位置参数中取第一个脚本扩展名者。
pub(crate) fn extract_script_arg(full_command: &str) -> Option<&str> {
    fn has_script_ext(tok: &str) -> bool {
        let lower = tok.to_lowercase();
        [".js", ".mjs", ".ts", ".cjs", ".py", ".rb", ".jar", ".php"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    }
    let mut args = full_command.split_whitespace().skip(1);
    while let Some(tok) = args.next() {
        if is_eval_mode_flag(tok) {
            return None; // 代码体的 token 不可信，整条放弃
        }
        if SCRIPT_VALUE_FLAGS.contains(&tok) {
            args.next(); // 消费分离值：可能是脚本路径，但不是入口
            continue;
        }
        if tok.starts_with('-') {
            continue; // 其余选项（含 --flag=value 粘连形）
        }
        if has_script_ext(tok) {
            return Some(tok);
        }
    }
    None
}

/// JVM 内存量值形态：可选单字母前缀 + 数字 + 可选 k/m/g 后缀（x512m、s256k、512m）。
/// 只用于把粘连 `-m` 的内存旗标排除出模块名 —— 真实 python 模块（http.server、
/// venv、pip）不落入此形态。
fn is_jvm_memory_value(v: &str) -> bool {
    let rest = v
        .strip_prefix(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(v);
    let rest = rest
        .strip_suffix(['k', 'K', 'm', 'M', 'g', 'G'])
        .unwrap_or(rest);
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

/// 从解释器命令行中提取 `-m <模块>` 调用（python -m http.server / -mhttp.server）。
/// 模块名是进程身份（类比脚本文件）—— 孤儿化的 `python -m http.server` 不能
/// 因解释器装在系统路径 / Homebrew 而被豁免（真实漏报案例）。
///
/// 扫描整个参数段而非「遇首个位置参数即停」：`-W ignore -m http.server` 的
/// 分离值 `ignore` 不带 `-`，按位置参数终止会漏掉模块（评审发现）。真正的
/// 脚本位置参数由 extract_script_arg 在两个调用点先行处理，互不冲突。
/// 例外：`-c` / `-e` / `-`（eval / stdin 模式）直接放弃 —— ps 会剥掉引号，
/// 代码体的 token 不可信，里面的 `-m` 不是模块调用。
pub(crate) fn extract_module_arg(full_command: &str) -> Option<&str> {
    let mut args = full_command.split_whitespace().skip(1);
    while let Some(tok) = args.next() {
        if is_eval_mode_flag(tok) {
            return None;
        }
        match tok {
            "-m" => return args.next(),
            _ => {
                if let Some(glued) = tok.strip_prefix("-m") {
                    // 粘连写法 `-mhttp.server`（python 支持）；排除 `--module-x` 类
                    // 长选项，以及 JVM 旧式内存旗标 —— `-mx512m`/`-ms256m`（-Xmx/-Xms
                    // 的历史别名，HotSpot 至今接受）形如合法模块名，曾被解析成
                    // 模块 "x512m" 并赋予 dev-script 身份（评审实锤）。
                    if !glued.is_empty() && !glued.starts_with('-') && !is_jvm_memory_value(glued) {
                        return Some(glued);
                    }
                }
            }
        }
    }
    None
}

/// 从路径推断「项目名」：
/// /Users/fhf/IT/code/portreaper/node_modules/... → "portreaper"
/// C:\Users\fhf\code\myapp\node_modules\...       → "myapp"
/// 规则：定位 Users 段，跳过用户名，取第一个停止词（node_modules/target/src/...）之前的目录名。
pub(crate) fn extract_project_name(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let users_idx = segments.iter().position(|s| *s == "Users")?;
    // Users/<用户名>/<之后才是项目相关路径>
    let path_segments = segments.get(users_idx + 2..)?;
    if path_segments.len() < 2 {
        return None;
    }
    let stop_words = [
        "node_modules",
        "target",
        "src",
        "src-tauri",
        ".bin",
        "dist",
        "build",
        ".venv",
        "venv",
        ".next",
        ".nuxt",
        "out",
    ];
    for (i, s) in path_segments.iter().enumerate() {
        if stop_words.contains(s) && i > 0 {
            return Some(path_segments[i - 1].to_string());
        }
    }
    None
}

/// 脚本运行时的标签合成：「项目 · 脚本」/「脚本 · 运行时」/「项目 · 运行时」。
/// 前端 AppLabel 按 " · " 拆成主/副两行渲染。
pub(crate) fn script_runtime_label(full_command: &str, short_command: &str) -> String {
    let script = extract_script_arg(full_command);
    let project = extract_project_name(full_command);
    match (script, project) {
        (Some(s), Some(p)) => format!("{} · {}", p, basename(s)),
        (Some(s), None) => format!("{} · {}", basename(s), short_command),
        (None, Some(p)) => format!("{} · {}", p, short_command),
        (None, None) => short_command.to_string(),
    }
}

/// short_command 是否为脚本运行时（node / python.exe / ...）。
pub(crate) fn is_script_runtime(short_command: &str) -> bool {
    let lower = short_command.to_lowercase();
    SCRIPT_RUNTIMES.contains(&strip_exe(&lower))
}

/// Cargo 产物 / 用户目录二进制的标签：「项目 · 二进制名」或裸名。
pub(crate) fn project_binary_label(exe_path: &str) -> String {
    let bin = strip_exe(basename(exe_path)).to_string();
    match extract_project_name(exe_path) {
        Some(p) if p != bin => format!("{} · {}", p, bin),
        _ => bin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_both_separators() {
        assert_eq!(basename("/usr/local/bin/node"), "node");
        assert_eq!(basename("C:\\Windows\\System32\\cmd.exe"), "cmd.exe");
        assert_eq!(basename("plain"), "plain");
    }

    #[test]
    fn dev_build_artifact_both_separators() {
        // cargo 产物 + go run 临时目录，双平台分隔符/大小写
        assert!(is_dev_build_artifact("/Users/x/p/target/debug/server"));
        assert!(is_dev_build_artifact("/Users/x/p/target/release/server"));
        assert!(is_dev_build_artifact(
            "/private/var/folders/dx/T/go-build123/b001/exe/main"
        ));
        assert!(is_dev_build_artifact(
            "C:\\Users\\x\\proj\\target\\debug\\server.exe"
        ));
        assert!(is_dev_build_artifact(
            "C:\\Users\\x\\AppData\\Local\\Temp\\go-build123\\b001\\exe\\server.exe"
        ));
        // 前导分隔符锚定：用户目录名内嵌 "go-build" 不误触
        assert!(!is_dev_build_artifact("/Users/x/my-go-build-tools/bin/app"));
        assert!(!is_dev_build_artifact("/Users/x/code/myproj/server"));
    }

    #[test]
    fn strip_exe_case_insensitive() {
        assert_eq!(strip_exe("node.exe"), "node");
        assert_eq!(strip_exe("Node.EXE"), "Node");
        assert_eq!(strip_exe("node"), "node");
    }

    #[test]
    fn project_name_macos_and_windows() {
        assert_eq!(
            extract_project_name("/Users/fhf/IT/code/portreaper/node_modules/.bin/vite"),
            Some("portreaper".to_string())
        );
        assert_eq!(
            extract_project_name("C:\\Users\\fhf\\code\\myapp\\node_modules\\.bin\\vite.cmd"),
            Some("myapp".to_string())
        );
        assert_eq!(
            extract_project_name("/Users/fhf/IT/rust/mytool/target/debug/mytool"),
            Some("mytool".to_string())
        );
        assert_eq!(extract_project_name("/opt/homebrew/bin/redis-server"), None);
    }

    #[test]
    fn script_label_composition() {
        assert_eq!(
            script_runtime_label(
                "/usr/local/bin/node /Users/x/proj/node_modules/vite/bin/vite.js --port 5173",
                "node"
            ),
            "proj · vite.js"
        );
        assert_eq!(
            script_runtime_label("node server.js", "node"),
            "server.js · node"
        );
    }

    /// 回归（评审实锤）：预加载/注册类选项的分离值不得被当作入口脚本 ——
    /// 否则标签、重复配对、brew 豁免三处全部用错身份（注入脚本恰在
    /// /opt/homebrew 内时会错误豁免一个真实孤儿 dev server）。
    #[test]
    fn script_arg_skips_flag_values() {
        // 分离值形：值是脚本路径但不是入口
        assert_eq!(
            extract_script_arg("node --import ./reg.js server.mjs"),
            Some("server.mjs")
        );
        assert_eq!(
            extract_script_arg("node -r ts-node/register --require ./reg.js server.ts"),
            Some("server.ts")
        );
        assert_eq!(
            extract_script_arg("node --loader ./loader.mjs app.ts"),
            Some("app.ts")
        );
        // 粘连形：整 token 以 - 开头，通用规则跳过
        assert_eq!(
            extract_script_arg("node --import=./reg.js server.mjs"),
            Some("server.mjs")
        );
        // 只有预加载、没有入口：不得把注入脚本当身份
        assert_eq!(
            extract_script_arg("node --import ./reg.js dist/server"),
            None
        );
        // --config 的分离值是配置不是入口：必须跳过它、取后面的真入口
        //（评审复核：移除 --config 才会把 app.config.js 误当入口 —— 锁死正确行为）
        assert_eq!(
            extract_script_arg("node --config app.config.js server.mjs"),
            Some("server.mjs")
        );
        // 非运行时程序仅给配置、无独立入口：取不到入口（身份回退到程序/项目名，无损）
        assert_eq!(extract_script_arg("vite --config vite.config.ts"), None);
        // 普通选项不受影响
        assert_eq!(
            extract_script_arg("python -W ignore app.py"),
            Some("app.py")
        );
        assert_eq!(extract_script_arg("java -jar app.jar"), Some("app.jar"));
        // classpath 家族的值是 jar/目录但不是入口（gradle daemon / tomcat 形态）
        assert_eq!(
            extract_script_arg("java -cp bootstrap.jar org.apache.catalina.startup.Bootstrap"),
            None
        );
        assert_eq!(
            extract_script_arg("java -classpath lib/app.jar com.example.Main"),
            None
        );
        assert_eq!(
            extract_script_arg("java --class-path a.jar --module-path mods com.example.Main"),
            None
        );
        // eval / stdin 模式熔断：去引号后代码体里的脚本扩展名 token 不可信
        assert_eq!(extract_script_arg("python -c import sys app.py"), None);
        assert_eq!(extract_script_arg("node -e require app.js"), None);
        assert_eq!(extract_script_arg("python - app.py"), None);
    }

    #[test]
    fn module_arg_extraction() {
        assert_eq!(
            extract_module_arg("/usr/bin/python3 -m http.server 8000"),
            Some("http.server")
        );
        assert_eq!(extract_module_arg("python -u -m flask run"), Some("flask"));
        // 分离值选项不终止扫描（评审发现的漏报变体）
        assert_eq!(
            extract_module_arg("python -W ignore -m http.server"),
            Some("http.server")
        );
        assert_eq!(
            extract_module_arg("python -X importtime -m http.server"),
            Some("http.server")
        );
        // 粘连写法
        assert_eq!(
            extract_module_arg("python -mhttp.server"),
            Some("http.server")
        );
        // eval / stdin 模式熔断：代码体里的 token 不可信
        assert_eq!(extract_module_arg("python -c print(1) -m x"), None);
        assert_eq!(extract_module_arg("perl -e exec -m x"), None);
        assert_eq!(extract_module_arg("python - -m x"), None);
        // 有脚本位置参数时调用点会先走 extract_script_arg；本函数全段扫描
        assert_eq!(
            extract_module_arg("python app.py -m something"),
            Some("something")
        );
        // 长选项不是粘连 -m
        assert_eq!(extract_module_arg("node --max-old-space-size=4096"), None);
        assert_eq!(extract_module_arg("python"), None);
        // JVM 旧式内存旗标不是模块名（-mx512m = -Xmx512m 的历史别名）
        assert_eq!(
            extract_module_arg("java -mx512m -cp classes com.Main"),
            None
        );
        assert_eq!(extract_module_arg("java -ms256m -mx1g com.Main"), None);
        // 形似但不是内存量值的粘连模块仍然命中
        assert_eq!(extract_module_arg("python -mvenv"), Some("venv"));
        assert_eq!(extract_module_arg("python -mpip install x"), Some("pip"));
    }

    /// KNOWN-GAPS Gap 1：一次性自动化实例的命令行判据。
    /// 真阳性必须检出，反例（尤其**有头**的活跃实例）必须一个都不命中 ——
    /// 误杀用户正在驱动的浏览器比漏报严重得多。
    #[test]
    fn automation_instance_requires_headless_plus_session_evidence() {
        // —— 真阳性：Gap 1 的实测主案与其变体 ——
        for tp in [
            // 主案：headless + 临时 profile + 调试端口
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --headless=new \
             --disable-gpu --user-data-dir=/private/tmp/claude-501/sess/scratchpad/cprof8 \
             --remote-debugging-port=9339 about:blank",
            // 只有调试端口（profile 在别处）
            "chrome --headless --remote-debugging-port=9222",
            // 只有临时 profile
            "chrome --headless=new --user-data-dir=/tmp/puppeteer_dev_profile-XYZ",
            // 调试管道（Playwright 默认通道，无端口）
            "chromium --headless --remote-debugging-pipe",
            // helper 子进程继承了同一批开关（主进程被杀后会被收养成孤儿）
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/\
             Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper \
             --type=gpu-process --headless=new --use-gl=disabled \
             --user-data-dir=/private/tmp/claude-501/sess/scratchpad/cprof8",
            // Windows 形态（%TEMP% 下的 profile，开关逐字相同）
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe --headless=new \
             --user-data-dir=C:\\Users\\x\\AppData\\Local\\Temp\\pptr_profile \
             --remote-debugging-port=9222",
            // Firefox：单横线 headless + marionette 通道
            "/Applications/Firefox.app/Contents/MacOS/firefox -headless -marionette \
             -profile /var/folders/xx/T/rust_mozprofile",
            // webdriver 驱动的实例
            "chrome --headless --enable-automation",
        ] {
            assert!(is_automation_instance(tp), "漏报: {tp}");
        }

        // —— 反例：一个都不能命中 ——
        for fp in [
            // A2 实测反例：**有头**的活跃实例（判据其余项全中）—— 用户正在用它
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
             --remote-debugging-port=9222 \
             --user-data-dir=/private/tmp/claude-501/sess/scratchpad/chrome-profile \
             --no-first-run --no-default-browser-check",
            // 日常浏览器：既无 headless 也无自动化证据
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            // headless 但 profile 是真实用户目录（长期自建的无头实例，不是一次性会话）
            "chrome --headless --user-data-dir=/Users/x/.config/my-scraper",
            // 光有 headless（可能是别的工具的无关开关），无会话证据
            "some-tool --headless",
            // 目录名里恰好含 headless 的普通进程
            "node /Users/x/code/headless-cms/server.js",
            // 单词前缀不得误命中（--headless-mode-off 这类拼接开关）
            "chrome --headlessx --remote-debugging-port=9222",
        ] {
            assert!(!is_automation_instance(fp), "误报: {fp}");
        }
    }

    /// 分离值形（`-profile /tmp/x`）与粘连形（`--user-data-dir=/tmp/x`）等价。
    #[test]
    fn automation_temp_profile_accepts_separated_value() {
        assert!(is_automation_instance(
            "firefox -headless -profile /tmp/prof"
        ));
        assert!(is_automation_instance(
            "firefox -headless --profile /private/var/folders/ab/T/prof"
        ));
        // 分离值不是临时目录 ⇒ 无证据
        assert!(!is_automation_instance(
            "firefox -headless -profile /Users/x/.mozilla/prof"
        ));
    }

    #[test]
    fn dev_tool_runtime_paths_both_separators() {
        // 项目本地 / 工具下载的浏览器 runtime —— 形态同真应用，但绝非用户安装
        assert!(is_dev_tool_runtime_path(
            "/Users/x/p/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron"
        ));
        assert!(is_dev_tool_runtime_path(
            "/Users/x/Library/Caches/ms-playwright/chromium-1148/chrome-mac/Chromium.app/Contents/MacOS/Chromium"
        ));
        assert!(is_dev_tool_runtime_path(
            "/Users/x/.cache/puppeteer/chrome/mac-131/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        ));
        assert!(is_dev_tool_runtime_path(
            "C:\\Users\\x\\AppData\\Local\\ms-playwright\\chromium-1148\\chrome-win\\chrome.exe"
        ));
        // 真安装的应用不得命中
        assert!(!is_dev_tool_runtime_path(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        ));
        assert!(!is_dev_tool_runtime_path(
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
        ));
    }

    #[test]
    fn automation_label_prefers_bundle_name() {
        assert_eq!(
            automation_label(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "Google Chrome"
            ),
            "Google Chrome · headless"
        );
        assert_eq!(
            automation_label(
                "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
                "chrome.exe"
            ),
            "chrome · headless"
        );
        // exe 读不到时退回短命令名
        assert_eq!(automation_label("", "chromium"), "chromium · headless");
    }

    #[test]
    fn script_runtime_detection() {
        assert!(is_script_runtime("node"));
        assert!(is_script_runtime("node.exe"));
        assert!(is_script_runtime("Python3"));
        assert!(!is_script_runtime("postgres"));
    }
}
