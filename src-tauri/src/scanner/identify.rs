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
    norm.contains("/target/debug/") || norm.contains("/target/release/") || norm.contains("/go-build")
}

/// 去掉 Windows 可执行后缀：node.exe → node（大小写不敏感）
pub(crate) fn strip_exe(name: &str) -> &str {
    let lower = name.to_lowercase();
    if lower.ends_with(".exe") {
        &name[..name.len() - 4]
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
                    // 粘连写法 `-mhttp.server`（python 支持）；排除 `--module-x` 类长选项
                    if !glued.is_empty() && !glued.starts_with('-') {
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
    }

    #[test]
    fn script_runtime_detection() {
        assert!(is_script_runtime("node"));
        assert!(is_script_runtime("node.exe"));
        assert!(is_script_runtime("Python3"));
        assert!(!is_script_runtime("postgres"));
    }
}
