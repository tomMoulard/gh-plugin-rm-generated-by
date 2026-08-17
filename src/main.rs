//! gh-plugin-rm-generated-by
//!
//! A gh CLI extension that removes AI-generated trailers (e.g.
//! "🤖 Generated with Claude Code" and "Co-authored-by: Claude ...")
//! from a pull request description.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process::{exit, Command};

const USAGE: &str = "\
gh-plugin-rm-generated-by

A gh CLI extension that removes AI-generated trailers (e.g.
\"🤖 Generated with Claude Code\" and \"Co-authored-by: Claude ...\")
from a pull request description.

Usage:
  gh plugin-rm-generated-by                 Clean the current branch's PR
  gh plugin-rm-generated-by <pr>            Clean a specific PR (number/url/branch)
  gh plugin-rm-generated-by --dry-run [pr]  Show what would change, don't edit
  gh plugin-rm-generated-by clean-all       Clean every open PR you authored (all repos)
                                            (accepts --dry-run and --limit <n>)
  gh plugin-rm-generated-by create [args]   Run `gh pr create` then clean the new PR
  gh plugin-rm-generated-by filter          Strip trailers from stdin -> stdout
  gh plugin-rm-generated-by shell-init      Print the `gh pr create` middleware fn
  gh plugin-rm-generated-by install [rc]    Install the middleware into your shell rc
";

const SHELL_INIT: &str = r#"# gh plugin-rm-generated-by — strip AI trailers right after `gh pr create`.
gh() {
  if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
    command gh "$@"
    local __rc=$?
    if [ "$__rc" -eq 0 ]; then
      command gh plugin-rm-generated-by 2>/dev/null || true
    fi
    return "$__rc"
  fi
  command gh "$@"
}
"#;

/// Returns `true` when a single line is an AI-generated trailer that should be
/// removed from a PR description.
fn is_trailer(line: &str) -> bool {
    let low = line.to_lowercase();
    // "🤖 Generated with ..." (emoji + generated marker)
    if line.contains('🤖') && low.contains("generated with") {
        return true;
    }
    // "Generated with ... Claude Code" (with or without the emoji)
    if low.contains("generated with") && low.contains("claude code") {
        return true;
    }
    // "Co-authored-by: Claude/Anthropic ..."
    let t = low.trim_start();
    t.starts_with("co-authored-by:") && (t.contains("claude") || t.contains("anthropic"))
}

/// Strips AI-generated trailer lines from a PR body. When something is removed,
/// trailing blank lines and a dangling horizontal rule (--- / *** / ___) left
/// behind by the trailer are trimmed too. If nothing matches, the body is
/// returned unchanged.
fn filter_body(body: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut deleted = false;

    for line in body.split('\n') {
        if is_trailer(line) {
            deleted = true;
        } else {
            kept.push(line);
        }
    }

    if deleted {
        while let Some(last) = kept.last() {
            let t = last.trim();
            if t.is_empty() || t == "---" || t == "***" || t == "___" {
                kept.pop();
            } else {
                break;
            }
        }
    }

    kept.join("\n")
}

/// Heuristic for retryable GitHub/API failures (5xx, timeouts, resets).
fn is_transient(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("503")
        || e.contains("502")
        || e.contains("504")
        || e.contains("no server is currently available")
        || e.contains("timeout")
        || e.contains("timed out")
        || e.contains("connection reset")
        || e.contains("temporarily")
}

/// Runs `gh` once, capturing (success, stdout, stderr).
fn gh_output_once(args: &[&str]) -> (bool, String, String) {
    match Command::new("gh").args(args).output() {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), format!("failed to run gh: {e}")),
    }
}

/// Runs `gh`, retrying up to 3 times with backoff on transient API failures.
fn gh_output(args: &[&str]) -> (bool, String, String) {
    let mut result = gh_output_once(args);
    let mut attempt = 1u64;
    while attempt < 3 && !result.0 && is_transient(&result.2) {
        std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
        result = gh_output_once(args);
        attempt += 1;
    }
    result
}

/// Cleans a single PR referenced by `target` (a number/url/branch, or `None`
/// for the current branch's PR). The same `target` is reused for both reading
/// and editing so URL-based targets hit the correct repository.
///
/// `preview` shows the full rewritten body on `--dry-run` (used for single-PR
/// runs); `label` overrides the "PR #<n>" prefix in messages (used by
/// clean-all to print the PR URL, since numbers collide across repos).
fn clean_target(target: Option<&str>, dry: bool, preview: bool, label: Option<&str>) -> i32 {
    // Resolve the PR number (defaults to the current branch's PR).
    let mut view_args: Vec<&str> = vec!["pr", "view"];
    if let Some(t) = target {
        view_args.push(t);
    }
    view_args.extend_from_slice(&["--json", "number", "-q", ".number"]);

    let (ok, out, err) = gh_output(&view_args);
    let number = out.trim().to_string();
    if !ok {
        // A real API/permission error — surface it instead of pretending the
        // PR does not exist.
        eprint!("{err}");
        match label.or(target) {
            Some(w) => eprintln!("error: failed to look up pull request '{w}'"),
            None => eprintln!("error: failed to look up pull request"),
        }
        return 1;
    }
    if number.is_empty() {
        match label.or(target) {
            Some(w) => eprintln!("error: no pull request found for '{w}' (are you on a PR branch?)"),
            None => eprintln!("error: no pull request found (are you on a PR branch?)"),
        }
        return 1;
    }
    let display = label
        .map(str::to_string)
        .unwrap_or_else(|| format!("PR #{number}"));

    let mut body_args: Vec<&str> = vec!["pr", "view"];
    if let Some(t) = target {
        body_args.push(t);
    }
    body_args.extend_from_slice(&["--json", "body", "-q", ".body"]);
    let (ok, body_raw, _) = gh_output(&body_args);
    if !ok {
        eprintln!("error: failed to read {display} body");
        return 1;
    }
    let body = body_raw.trim_end_matches('\n');
    let filtered = filter_body(body);

    if filtered == body {
        println!("{display}: no AI-generated trailer found; nothing to remove.");
        return 0;
    }

    if dry {
        if preview {
            println!("{display}: would rewrite the description to:");
            println!("----------8<----------");
            println!("{filtered}");
            println!("---------->8----------");
        } else {
            println!("{display}: would remove AI-generated trailer (dry-run).");
        }
        return 0;
    }

    let mut edit_args: Vec<&str> = vec!["pr", "edit"];
    if let Some(t) = target {
        edit_args.push(t);
    }
    edit_args.extend_from_slice(&["--body", &filtered]);
    let (ok, _, err) = gh_output(&edit_args);
    if ok {
        println!("{display}: removed AI-generated trailer from the description.");
        0
    } else {
        eprint!("{err}");
        1
    }
}

fn cmd_clean(args: &[String]) -> i32 {
    let mut dry = false;
    let mut target: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--dry-run" | "-n" => dry = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => target = Some(other),
        }
    }
    clean_target(target, dry, true, None)
}

fn cmd_clean_all(args: &[String]) -> i32 {
    let mut dry = false;
    let mut limit = String::from("100");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" | "-n" => dry = true,
            "--limit" => {
                i += 1;
                match args.get(i) {
                    Some(v) => limit = v.clone(),
                    None => {
                        eprintln!("clean-all: --limit requires a value");
                        return 2;
                    }
                }
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("clean-all: unexpected argument '{other}'");
                return 2;
            }
        }
        i += 1;
    }

    // GitHub search understands `@me`, so we avoid depending on the `/user`
    // endpoint (which some environments/proxies block with a 503).
    let (ok, out, err) = gh_output(&[
        "search", "prs", "--author=@me", "--state=open", "--limit", &limit, "--json", "url", "-q",
        ".[].url",
    ]);
    if !ok {
        eprint!("{err}");
        return 1;
    }

    let urls: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if urls.is_empty() {
        println!("No open pull requests authored by you were found.");
        return 0;
    }

    println!("Found {} open PR(s) you authored.", urls.len());
    let mut failures = 0;
    for url in urls {
        if clean_target(Some(url), dry, false, Some(url)) != 0 {
            failures += 1;
        }
    }
    if failures > 0 {
        eprintln!("clean-all: {failures} PR(s) could not be processed.");
        1
    } else {
        0
    }
}

fn cmd_create(args: &[String]) -> i32 {
    let status = Command::new("gh")
        .arg("pr")
        .arg("create")
        .args(args)
        .status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("failed to run gh: {e}");
            return 1;
        }
    };
    if code != 0 {
        return code;
    }
    // `--web` returns before the PR exists; ignore "no PR found" in that case.
    let _ = clean_target(None, false, false, None);
    0
}

fn cmd_filter() -> i32 {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("error: failed to read stdin");
        return 1;
    }
    let body = input.trim_end_matches('\n');
    println!("{}", filter_body(body));
    0
}

fn cmd_install(args: &[String]) -> i32 {
    let rc = match args.first() {
        Some(path) => path.clone(),
        None => {
            let shell = env::var("SHELL").unwrap_or_default();
            let base = shell.rsplit('/').next().unwrap_or("");
            let home = env::var("HOME").unwrap_or_default();
            match base {
                "zsh" => format!("{home}/.zshrc"),
                "bash" => format!("{home}/.bashrc"),
                _ => format!("{home}/.profile"),
            }
        }
    };

    let line = "eval \"$(gh plugin-rm-generated-by shell-init)\"";
    if let Ok(existing) = fs::read_to_string(&rc) {
        if existing.contains(line) {
            println!("Middleware already installed in {rc}");
            return 0;
        }
    }

    let block = format!("\n# gh plugin-rm-generated-by middleware\n{line}\n");
    let open = fs::OpenOptions::new().create(true).append(true).open(&rc);
    match open {
        Ok(mut f) => {
            if let Err(e) = f.write_all(block.as_bytes()) {
                eprintln!("error: failed to write {rc}: {e}");
                return 1;
            }
        }
        Err(e) => {
            eprintln!("error: failed to open {rc}: {e}");
            return 1;
        }
    }

    println!("Installed the gh pr create middleware into {rc}");
    println!("Restart your shell or run:  source \"{rc}\"");
    0
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("create") => cmd_create(&args[1..]),
        Some("clean-all") | Some("all") => cmd_clean_all(&args[1..]),
        Some("filter") => cmd_filter(),
        Some("shell-init") => {
            print!("{SHELL_INIT}");
            0
        }
        Some("install") => cmd_install(&args[1..]),
        Some("-h") | Some("--help") | Some("help") | None => {
            print!("{USAGE}");
            0
        }
        _ => cmd_clean(&args),
    };
    exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_claude_code_trailer_and_coauthor() {
        let body = "Implement the widget cache.\n\nImproves cold-start latency.\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>";
        assert_eq!(
            filter_body(body),
            "Implement the widget cache.\n\nImproves cold-start latency."
        );
    }

    #[test]
    fn removes_dangling_horizontal_rule() {
        let body = "Fix the flaky test.\n\n---\n🤖 Generated with [Claude Code](https://claude.com/claude-code)";
        assert_eq!(filter_body(body), "Fix the flaky test.");
    }

    #[test]
    fn leaves_normal_body_unchanged() {
        let body = "Just a normal PR body.\n\nWith two paragraphs.";
        assert_eq!(filter_body(body), body);
    }

    #[test]
    fn keeps_non_claude_coauthors() {
        let body = "Work.\n\nCo-authored-by: Jane Dev <jane@example.com>";
        assert_eq!(filter_body(body), body);
    }

    #[test]
    fn transient_errors_are_detected() {
        assert!(is_transient(
            "non-200 OK status code: 503 Service Unavailable"
        ));
        assert!(is_transient("No server is currently available"));
        assert!(!is_transient("HTTP 404: Not Found"));
        assert!(!is_transient("could not resolve to a PullRequest"));
    }
}
