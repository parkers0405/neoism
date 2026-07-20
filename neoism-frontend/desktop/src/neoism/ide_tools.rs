use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::ffi::OsString;

#[derive(Clone, Copy, Debug)]
pub struct TreesitterInstallSpec {
    pub lang: &'static str,
    pub display_name: &'static str,
    pub repo: &'static str,
    pub subdir: &'static str,
    pub branch: Option<&'static str>,
}

pub const TREESITTER_INSTALL_SPECS: &[TreesitterInstallSpec] = &[
    TreesitterInstallSpec {
        lang: "rust",
        display_name: "Rust",
        repo: "https://github.com/tree-sitter/tree-sitter-rust",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "python",
        display_name: "Python",
        repo: "https://github.com/tree-sitter/tree-sitter-python",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "javascript",
        display_name: "JavaScript",
        repo: "https://github.com/tree-sitter/tree-sitter-javascript",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "typescript",
        display_name: "TypeScript",
        repo: "https://github.com/tree-sitter/tree-sitter-typescript",
        subdir: "typescript",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "tsx",
        display_name: "TSX",
        repo: "https://github.com/tree-sitter/tree-sitter-typescript",
        subdir: "tsx",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "go",
        display_name: "Go",
        repo: "https://github.com/tree-sitter/tree-sitter-go",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "lua",
        display_name: "Lua",
        repo: "https://github.com/MunifTanjim/tree-sitter-lua",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "json",
        display_name: "JSON",
        repo: "https://github.com/tree-sitter/tree-sitter-json",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "toml",
        display_name: "TOML",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-toml",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "yaml",
        display_name: "YAML",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-yaml",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "markdown",
        display_name: "Markdown",
        repo: "https://github.com/MDeiml/tree-sitter-markdown",
        subdir: "tree-sitter-markdown",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "nix",
        display_name: "Nix",
        repo: "https://github.com/nix-community/tree-sitter-nix",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "bash",
        display_name: "Bash",
        repo: "https://github.com/tree-sitter/tree-sitter-bash",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "c",
        display_name: "C",
        repo: "https://github.com/tree-sitter/tree-sitter-c",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "cpp",
        display_name: "C++",
        repo: "https://github.com/tree-sitter/tree-sitter-cpp",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "c_sharp",
        display_name: "C#",
        repo: "https://github.com/tree-sitter/tree-sitter-c-sharp",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "java",
        display_name: "Java",
        repo: "https://github.com/tree-sitter/tree-sitter-java",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "kotlin",
        display_name: "Kotlin",
        repo: "https://github.com/fwcd/tree-sitter-kotlin",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "scala",
        display_name: "Scala",
        repo: "https://github.com/tree-sitter/tree-sitter-scala",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "zig",
        display_name: "Zig",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-zig",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "dart",
        display_name: "Dart",
        repo: "https://github.com/UserNobody14/tree-sitter-dart",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "ruby",
        display_name: "Ruby",
        repo: "https://github.com/tree-sitter/tree-sitter-ruby",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "php",
        display_name: "PHP",
        repo: "https://github.com/tree-sitter/tree-sitter-php",
        subdir: "php",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "perl",
        display_name: "Perl",
        repo: "https://github.com/tree-sitter-perl/tree-sitter-perl",
        subdir: ".",
        branch: Some("release"),
    },
    TreesitterInstallSpec {
        lang: "elixir",
        display_name: "Elixir",
        repo: "https://github.com/elixir-lang/tree-sitter-elixir",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "erlang",
        display_name: "Erlang",
        repo: "https://github.com/WhatsApp/tree-sitter-erlang",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "gleam",
        display_name: "Gleam",
        repo: "https://github.com/gleam-lang/tree-sitter-gleam",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "haskell",
        display_name: "Haskell",
        repo: "https://github.com/tree-sitter/tree-sitter-haskell",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "ocaml",
        display_name: "OCaml",
        repo: "https://github.com/tree-sitter/tree-sitter-ocaml",
        subdir: "grammars/ocaml",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "fsharp",
        display_name: "F#",
        repo: "https://github.com/ionide/tree-sitter-fsharp",
        subdir: "fsharp",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "fortran",
        display_name: "Fortran",
        repo: "https://github.com/stadelmanma/tree-sitter-fortran",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "clojure",
        display_name: "Clojure",
        repo: "https://github.com/sogaiu/tree-sitter-clojure",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "r",
        display_name: "R",
        repo: "https://github.com/r-lib/tree-sitter-r",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "julia",
        display_name: "Julia",
        repo: "https://github.com/tree-sitter/tree-sitter-julia",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "elm",
        display_name: "Elm",
        repo: "https://github.com/elm-tooling/tree-sitter-elm",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "gdscript",
        display_name: "GDScript",
        repo: "https://github.com/PrestonKnopp/tree-sitter-gdscript",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "godot_resource",
        display_name: "Godot Resources",
        repo: "https://github.com/PrestonKnopp/tree-sitter-godot-resource",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "html",
        display_name: "HTML",
        repo: "https://github.com/tree-sitter/tree-sitter-html",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "css",
        display_name: "CSS",
        repo: "https://github.com/tree-sitter/tree-sitter-css",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "scss",
        display_name: "SCSS",
        repo: "https://github.com/serenadeai/tree-sitter-scss",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "astro",
        display_name: "Astro",
        repo: "https://github.com/virchau13/tree-sitter-astro",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "svelte",
        display_name: "Svelte",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-svelte",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "vue",
        display_name: "Vue",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-vue",
        subdir: ".",
        branch: Some("main"),
    },
    TreesitterInstallSpec {
        lang: "dockerfile",
        display_name: "Dockerfile",
        repo: "https://github.com/camdencheek/tree-sitter-dockerfile",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "make",
        display_name: "Make",
        repo: "https://github.com/alemuller/tree-sitter-make",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "cmake",
        display_name: "CMake",
        repo: "https://github.com/uyha/tree-sitter-cmake",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "sql",
        display_name: "SQL",
        repo: "https://github.com/derekstride/tree-sitter-sql",
        subdir: ".",
        branch: Some("gh-pages"),
    },
    TreesitterInstallSpec {
        lang: "graphql",
        display_name: "GraphQL",
        repo: "https://github.com/bkegley/tree-sitter-graphql",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "prisma",
        display_name: "Prisma",
        repo: "https://github.com/victorhqc/tree-sitter-prisma",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "proto",
        display_name: "Protocol Buffers",
        repo: "https://github.com/treywood/tree-sitter-proto",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "hcl",
        display_name: "HCL",
        repo: "https://github.com/MichaHoffmann/tree-sitter-hcl",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "terraform",
        display_name: "Terraform",
        repo: "https://github.com/MichaHoffmann/tree-sitter-hcl",
        subdir: "dialects/terraform",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "ini",
        display_name: "INI",
        repo: "https://github.com/justinmk/tree-sitter-ini",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "csv",
        display_name: "CSV",
        repo: "https://github.com/amaanq/tree-sitter-csv",
        subdir: "csv",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "diff",
        display_name: "Diff",
        repo: "https://github.com/the-mikedavis/tree-sitter-diff",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "xml",
        display_name: "XML",
        repo: "https://github.com/tree-sitter-grammars/tree-sitter-xml",
        subdir: "xml",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "query",
        display_name: "Tree-sitter Query",
        repo: "https://github.com/nvim-treesitter/tree-sitter-query",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "regex",
        display_name: "Regex",
        repo: "https://github.com/tree-sitter/tree-sitter-regex",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "vim",
        display_name: "Vimscript",
        repo: "https://github.com/neovim/tree-sitter-vim",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "vimdoc",
        display_name: "Vim Help",
        repo: "https://github.com/neovim/tree-sitter-vimdoc",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "powershell",
        display_name: "PowerShell",
        repo: "https://github.com/airbus-cert/tree-sitter-powershell",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "typst",
        display_name: "Typst",
        repo: "https://github.com/uben0/tree-sitter-typst",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "glsl",
        display_name: "GLSL",
        repo: "https://github.com/theHamsta/tree-sitter-glsl",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "wgsl",
        display_name: "WGSL",
        repo: "https://github.com/szebniok/tree-sitter-wgsl",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "bibtex",
        display_name: "BibTeX",
        repo: "https://github.com/latex-lsp/tree-sitter-bibtex",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "json5",
        display_name: "JSON5",
        repo: "https://github.com/Joakker/tree-sitter-json5",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "jsdoc",
        display_name: "JSDoc",
        repo: "https://github.com/tree-sitter/tree-sitter-jsdoc",
        subdir: ".",
        branch: None,
    },
    TreesitterInstallSpec {
        lang: "just",
        display_name: "Just",
        repo: "https://github.com/IndianBoy42/tree-sitter-just",
        subdir: ".",
        branch: None,
    },
];

pub fn treesitter_install_specs() -> &'static [TreesitterInstallSpec] {
    TREESITTER_INSTALL_SPECS
}

pub fn treesitter_install_spec(lang: &str) -> Option<TreesitterInstallSpec> {
    TREESITTER_INSTALL_SPECS
        .iter()
        .copied()
        .find(|spec| spec.lang == lang)
}

pub fn agent_command_exists(binary: &str) -> bool {
    command_exists(binary)
}

#[derive(Clone, Copy, Debug)]
pub struct AgentInstallSpec {
    pub binary: &'static str,
    pub display_name: &'static str,
    pub manager: &'static str,
}

pub fn agent_install_spec(id: &str) -> Option<AgentInstallSpec> {
    match id {
        "claude" => Some(AgentInstallSpec {
            binary: "claude",
            display_name: "Claude Code",
            manager: "npm",
        }),
        "codex" => Some(AgentInstallSpec {
            binary: "codex",
            display_name: "Codex",
            manager: "npm",
        }),
        "opencode" => Some(AgentInstallSpec {
            binary: "opencode",
            display_name: "OpenCode",
            manager: "curl | bash",
        }),
        _ => None,
    }
}

pub fn install_agent(id: &str) -> Result<String, String> {
    let spec = agent_install_spec(id)
        .ok_or_else(|| format!("Neoism does not know how to install `{id}`."))?;
    let message = match id {
        "claude" => install_npm_global(&spec, "@anthropic-ai/claude-code"),
        "codex" => install_npm_global(&spec, "@openai/codex"),
        "opencode" => install_via_shell_pipe(&spec, "https://opencode.ai/install"),
        _ => Err(format!("unsupported agent `{id}`")),
    }?;

    if !command_exists(spec.binary) {
        return Err(format!(
            "{} install finished, but `{}` is still not on PATH. The package may have installed somewhere not on your shell PATH — open a new shell or extend PATH and retry.",
            spec.display_name, spec.binary,
        ));
    }
    Ok(format!(
        "{message}\n\nVerified `{}` is on PATH.",
        spec.binary
    ))
}

/// Install a package globally via the user's existing `npm`. Uses the
/// default global prefix (whatever `npm config get prefix` returns)
/// so the binary lands somewhere the user's shell already discovers
/// — same strategy `npm install -g @foo/bar` uses by hand.
fn install_npm_global(spec: &AgentInstallSpec, package: &str) -> Result<String, String> {
    ensure_command(
        "npm",
        "Install Node.js/npm first, then retry the install from Neoism.",
    )?;
    let mut cmd = Command::new("npm");
    cmd.arg("install").arg("-g").arg(package);
    run_command(&mut cmd, &format!("npm install -g {package}"))?;
    Ok(format!(
        "Installed {} via `npm install -g {package}`.",
        spec.display_name,
    ))
}

/// `curl -fsSL <url> | bash` — the de-facto install path for tools
/// like opencode that ship a hosted shell installer. We don't shell
/// out to a literal pipe; instead we read the body with curl, then
/// hand it to bash on stdin.
fn install_via_shell_pipe(spec: &AgentInstallSpec, url: &str) -> Result<String, String> {
    ensure_command(
        "curl",
        "Install curl first, then retry the install from Neoism.",
    )?;
    ensure_command(
        "bash",
        "bash is required to run the upstream installer script.",
    )?;
    let curl_out = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .map_err(|err| format!("curl failed: {err}"))?;
    if !curl_out.status.success() {
        return Err(format!(
            "curl could not fetch {url}: {}",
            String::from_utf8_lossy(&curl_out.stderr).trim()
        ));
    }
    use std::io::Write;
    let mut child = Command::new("bash")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("bash spawn failed: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&curl_out.stdout)
            .map_err(|err| format!("failed to feed installer to bash: {err}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("bash wait failed: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} installer exited with status {}: {}",
            spec.display_name,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(format!(
        "Ran the upstream {} installer from {url}.",
        spec.display_name
    ))
}

pub fn install_treesitter_parser(lang: &str) -> Result<String, String> {
    let spec = treesitter_install_spec(lang).ok_or_else(|| {
        format!("Neoism does not have a Treesitter installer for `{lang}` yet.")
    })?;

    ensure_command(
        "git",
        "Install git first, then retry Treesitter parser install.",
    )?;
    let source_root = neoism_backend::performer::nvim::rio_lsp_root_dir()
        .join("treesitter-src")
        .join(spec.lang);
    let build_root = neoism_backend::performer::nvim::rio_lsp_root_dir()
        .join("treesitter-build")
        .join(spec.lang);
    let parser_dir = neoism_backend::performer::nvim::rio_nvim_parser_dir();
    fs::create_dir_all(&build_root)
        .map_err(|err| format!("failed to create Treesitter build dir: {err}"))?;
    fs::create_dir_all(&parser_dir)
        .map_err(|err| format!("failed to create Treesitter parser dir: {err}"))?;

    if source_root.join(".git").is_dir() {
        run_command(
            Command::new("git")
                .arg("-C")
                .arg(&source_root)
                .args(["pull", "--ff-only"]),
            "git pull Treesitter grammar",
        )?;
    } else {
        if let Some(parent) = source_root.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!("failed to create Treesitter source dir: {err}")
            })?;
        }
        let mut clone = Command::new("git");
        clone.args(["clone", "--depth", "1"]);
        if let Some(branch) = spec.branch {
            clone.args(["--branch", branch]);
        }
        clone.arg(spec.repo).arg(&source_root);
        run_command(&mut clone, "git clone Treesitter grammar")?;
    }

    let grammar_root = if spec.subdir == "." {
        source_root.clone()
    } else {
        source_root.join(spec.subdir)
    };
    let src_dir = grammar_root.join("src");
    let parser_c = src_dir.join("parser.c");
    if !parser_c.is_file() {
        return Err(format!(
            "{} does not contain src/parser.c",
            grammar_root.display()
        ));
    }

    let output = parser_dir.join(format!("{}.so", spec.lang));
    if command_exists("tree-sitter") {
        run_command(
            Command::new("tree-sitter")
                .arg("build")
                .arg("--output")
                .arg(&output)
                .arg(&grammar_root),
            "tree-sitter build parser",
        )?;
        copy_treesitter_queries(&source_root, &grammar_root, spec.lang)?;
        return Ok(format!(
            "Installed {} Treesitter parser and highlight queries into {}.",
            spec.display_name,
            output.display()
        ));
    }

    ensure_command(
        "cc",
        "Install tree-sitter CLI or a C compiler first, then retry Treesitter parser install.",
    )?;

    let parser_o = build_root.join("parser.o");
    compile_c(&parser_c, &parser_o, &src_dir)?;
    let mut objects = vec![parser_o];
    let mut needs_cxx = false;

    let scanner_c = src_dir.join("scanner.c");
    if scanner_c.is_file() {
        let scanner_o = build_root.join("scanner.o");
        compile_c(&scanner_c, &scanner_o, &src_dir)?;
        objects.push(scanner_o);
    }

    let scanner_cc = src_dir.join("scanner.cc");
    if scanner_cc.is_file() {
        ensure_command(
            "c++",
            "Install a C++ compiler first, then retry Treesitter parser install.",
        )?;
        let scanner_o = build_root.join("scanner_cc.o");
        compile_cxx(&scanner_cc, &scanner_o, &src_dir)?;
        objects.push(scanner_o);
        needs_cxx = true;
    }

    link_shared(&objects, &output, needs_cxx)?;
    copy_treesitter_queries(&source_root, &grammar_root, spec.lang)?;
    Ok(format!(
        "Installed {} Treesitter parser and highlight queries into {}.",
        spec.display_name,
        output.display()
    ))
}

fn copy_treesitter_queries(
    source_root: &Path,
    grammar_root: &Path,
    lang: &str,
) -> Result<(), String> {
    let source_queries = query_source_dir(source_root, grammar_root, lang)?;
    if !source_queries.is_dir() {
        return Err(format!(
            "{} does not contain Treesitter highlight queries",
            grammar_root.display()
        ));
    }

    if !source_queries.join("highlights.scm").is_file() {
        return Err(format!(
            "{} does not contain queries/highlights.scm",
            grammar_root.display()
        ));
    }

    let dest_queries = neoism_backend::performer::nvim::rio_nvim_runtime_dir()
        .join("queries")
        .join(lang);
    fs::create_dir_all(&dest_queries)
        .map_err(|err| format!("failed to create Treesitter query dir: {err}"))?;

    let highlights = merged_highlight_query(source_root, &source_queries, lang)?;
    fs::write(dest_queries.join("highlights.scm"), highlights)
        .map_err(|err| format!("failed to write merged Treesitter highlights: {err}"))?;

    for entry in fs::read_dir(&source_queries)
        .map_err(|err| format!("failed to read Treesitter query dir: {err}"))?
    {
        let entry = entry
            .map_err(|err| format!("failed to read Treesitter query entry: {err}"))?;
        let src = entry.path();
        if !src.is_file() || src.extension().and_then(|ext| ext.to_str()) != Some("scm") {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy() == "highlights.scm" {
            continue;
        }
        fs::copy(&src, dest_queries.join(&name)).map_err(|err| {
            format!(
                "failed to copy Treesitter query {}: {err}",
                name.to_string_lossy()
            )
        })?;
    }

    Ok(())
}

fn query_source_dir(
    source_root: &Path,
    grammar_root: &Path,
    lang: &str,
) -> Result<PathBuf, String> {
    let grammar_queries = grammar_root.join("queries");
    if grammar_queries.join("highlights.scm").is_file() {
        return Ok(grammar_queries);
    }

    // Multi-grammar repos like tree-sitter-typescript keep parser sources in
    // per-language subdirs (`typescript/`, `tsx/`) but share queries at the
    // repository root.
    let root_queries = source_root.join("queries");
    if root_queries.join("highlights.scm").is_file() {
        return Ok(root_queries);
    }

    let nvim_queries = ensure_nvim_treesitter_queries()?.join(lang);
    if nvim_queries.is_dir() {
        return Ok(nvim_queries);
    }

    Ok(grammar_queries)
}

fn ensure_nvim_treesitter_queries() -> Result<PathBuf, String> {
    let source_root = neoism_backend::performer::nvim::rio_lsp_root_dir()
        .join("treesitter-src")
        .join("nvim-treesitter");
    if source_root.join(".git").is_dir() {
        run_command(
            Command::new("git")
                .arg("-C")
                .arg(&source_root)
                .args(["pull", "--ff-only"]),
            "git pull nvim-treesitter queries",
        )?;
    } else {
        if let Some(parent) = source_root.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!("failed to create nvim-treesitter source dir: {err}")
            })?;
        }
        run_command(
            Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "https://github.com/nvim-treesitter/nvim-treesitter",
                ])
                .arg(&source_root),
            "git clone nvim-treesitter queries",
        )?;
    }
    Ok(source_root.join("queries"))
}

fn merged_highlight_query(
    source_root: &Path,
    source_queries: &Path,
    lang: &str,
) -> Result<String, String> {
    let mut files = vec![source_queries.join("highlights.scm")];
    let sibling_javascript_queries = source_root
        .parent()
        .map(|parent| parent.join("javascript").join("queries"));
    let nvim_queries_root = neoism_backend::performer::nvim::rio_lsp_root_dir()
        .join("treesitter-src")
        .join("nvim-treesitter")
        .join("queries");

    match lang {
        // tree-sitter-javascript publishes JSX and parameter captures as
        // separate highlight fragments, but Neovim loads `highlights.scm`.
        // Fold them into the installed file so `.jsx` actually gets JSX tags.
        "javascript" => {
            files.push(source_queries.join("highlights-jsx.scm"));
            files.push(source_queries.join("highlights-params.scm"));
        }
        // tree-sitter-typescript keeps TypeScript-specific queries in its own
        // repo and references JavaScript queries for the JS expression grammar.
        "typescript" => {
            if let Some(js_queries) = sibling_javascript_queries.as_ref() {
                files.push(js_queries.join("highlights.scm"));
            }
            files.push(nvim_queries_root.join("javascript").join("highlights.scm"));
        }
        "tsx" => {
            if let Some(js_queries) = sibling_javascript_queries.as_ref() {
                files.push(js_queries.join("highlights-jsx.scm"));
                files.push(js_queries.join("highlights.scm"));
            }
            files.push(
                nvim_queries_root
                    .join("javascript")
                    .join("highlights-jsx.scm"),
            );
            files.push(nvim_queries_root.join("javascript").join("highlights.scm"));
        }
        _ => {}
    }

    let mut merged = String::new();
    for file in files {
        if !file.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&file).map_err(|err| {
            format!(
                "failed to read Treesitter highlight query {}: {err}",
                file.display()
            )
        })?;
        merged.push_str("\n; ---- ");
        merged.push_str(&file.display().to_string());
        merged.push_str(" ----\n");
        merged.push_str(&contents);
        if !contents.ends_with('\n') {
            merged.push('\n');
        }
    }

    if merged.trim().is_empty() {
        return Err(format!(
            "{} does not contain queries/highlights.scm",
            source_queries.display()
        ));
    }

    Ok(merged)
}

fn compile_c(source: &Path, output: &Path, include_dir: &Path) -> Result<(), String> {
    run_command(
        Command::new("cc")
            .arg("-fPIC")
            .arg("-O2")
            .arg("-I")
            .arg(include_dir)
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(output),
        "compile Treesitter C source",
    )
}

fn compile_cxx(source: &Path, output: &Path, include_dir: &Path) -> Result<(), String> {
    run_command(
        Command::new("c++")
            .arg("-fPIC")
            .arg("-O2")
            .arg("-I")
            .arg(include_dir)
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(output),
        "compile Treesitter C++ source",
    )
}

fn link_shared(
    objects: &[PathBuf],
    output: &Path,
    needs_cxx: bool,
) -> Result<(), String> {
    let linker = if needs_cxx { "c++" } else { "cc" };
    let mut cmd = Command::new(linker);
    cmd.arg("-shared").arg("-o").arg(output);
    for object in objects {
        cmd.arg(object);
    }
    run_command(&mut cmd, "link Treesitter parser")
}

fn run_command(cmd: &mut Command, label: &str) -> Result<(), String> {
    let output = cmd
        .output()
        .map_err(|err| format!("failed to run {label}: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };
    Err(format!("{label} failed: {details}"))
}

fn ensure_command(command: &str, missing_message: &str) -> Result<(), String> {
    if command_exists(command) {
        Ok(())
    } else {
        Err(missing_message.to_string())
    }
}

fn command_exists(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| executable_candidate(&dir, command))
}

#[cfg(windows)]
fn executable_candidate(dir: &Path, command: &str) -> bool {
    let pathext =
        std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".EXE;.CMD;.BAT"));
    let exts = pathext.to_string_lossy();
    exts.split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .any(|ext| dir.join(format!("{command}{ext}")).is_file())
        || dir.join(command).is_file()
}

#[cfg(not(windows))]
fn executable_candidate(dir: &Path, command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(command);
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}
