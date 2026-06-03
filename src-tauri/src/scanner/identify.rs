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
    fn script_runtime_detection() {
        assert!(is_script_runtime("node"));
        assert!(is_script_runtime("node.exe"));
        assert!(is_script_runtime("Python3"));
        assert!(!is_script_runtime("postgres"));
    }
}
