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

/// 去掉 Windows 可执行后缀：node.exe → node（大小写不敏感）
pub(crate) fn strip_exe(name: &str) -> &str {
    let lower = name.to_lowercase();
    if lower.ends_with(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    }
}

/// 从完整命令行中找第一个脚本参数（.js/.ts/.py/...）。
pub(crate) fn extract_script_arg(full_command: &str) -> Option<&str> {
    full_command.split_whitespace().skip(1).find(|a| {
        let lower = a.to_lowercase();
        lower.ends_with(".js")
            || lower.ends_with(".mjs")
            || lower.ends_with(".ts")
            || lower.ends_with(".cjs")
            || lower.ends_with(".py")
            || lower.ends_with(".rb")
            || lower.ends_with(".jar")
            || lower.ends_with(".php")
    })
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
        match tok {
            "-c" | "-e" | "-" => return None,
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
