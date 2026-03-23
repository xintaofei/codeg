use crate::acp::registry::PlatformBinary;

#[derive(Debug, Clone)]
pub enum LspDistribution {
    Npm {
        version: &'static str,
        package: &'static str,
        cmd: &'static str,
        args: &'static [&'static str],
        node_required: Option<&'static str>,
    },
    Binary {
        version: &'static str,
        cmd: &'static str,
        platforms: &'static [PlatformBinary],
    },
    CargoInstall {
        version: &'static str,
        crate_name: &'static str,
        cmd: &'static str,
        features: &'static [&'static str],
    },
    PipInstall {
        version: &'static str,
        package: &'static str,
        cmd: &'static str,
        python_required: Option<&'static str>,
    },
}

#[derive(Debug, Clone)]
pub struct LspServerMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub language: &'static str,
    pub distribution: LspDistribution,
}

impl LspServerMeta {
    pub fn registry_version(&self) -> Option<&'static str> {
        match &self.distribution {
            LspDistribution::Npm { version, .. }
            | LspDistribution::Binary { version, .. }
            | LspDistribution::CargoInstall { version, .. }
            | LspDistribution::PipInstall { version, .. } => Some(*version),
        }
    }

    pub fn distribution_type(&self) -> &'static str {
        match &self.distribution {
            LspDistribution::Npm { .. } => "npm",
            LspDistribution::Binary { .. } => "binary",
            LspDistribution::CargoInstall { .. } => "cargo_install",
            LspDistribution::PipInstall { .. } => "pip_install",
        }
    }
}

pub fn all_lsp_servers() -> Vec<&'static str> {
    vec![
        "typescript_language_server",
        "vscode_langservers",
        "bash_language_server",
        "yaml_language_server",
        "pyright",
        "rust_analyzer",
        "gopls",
        "clangd",
        "lua_language_server",
        "taplo",
    ]
}

pub fn get_lsp_meta(id: &str) -> Option<LspServerMeta> {
    match id {
        "typescript_language_server" => Some(LspServerMeta {
            id: "typescript_language_server",
            name: "TypeScript Language Server",
            description: "Language server for TypeScript and JavaScript",
            language: "TypeScript/JavaScript",
            distribution: LspDistribution::Npm {
                version: "4.3.3",
                package: "typescript-language-server@4.3.3",
                cmd: "typescript-language-server",
                args: &["--stdio"],
                node_required: None,
            },
        }),
        "vscode_langservers" => Some(LspServerMeta {
            id: "vscode_langservers",
            name: "vscode-langservers-extracted",
            description: "HTML/CSS/JSON/ESLint language servers extracted from VS Code",
            language: "HTML/CSS/JSON",
            distribution: LspDistribution::Npm {
                version: "4.10.0",
                package: "vscode-langservers-extracted@4.10.0",
                cmd: "vscode-json-language-server",
                args: &["--stdio"],
                node_required: None,
            },
        }),
        "bash_language_server" => Some(LspServerMeta {
            id: "bash_language_server",
            name: "Bash Language Server",
            description: "Language server for Bash shell scripts",
            language: "Shell",
            distribution: LspDistribution::Npm {
                version: "5.4.3",
                package: "bash-language-server@5.4.3",
                cmd: "bash-language-server",
                args: &["start"],
                node_required: None,
            },
        }),
        "yaml_language_server" => Some(LspServerMeta {
            id: "yaml_language_server",
            name: "YAML Language Server",
            description: "Language server for YAML files with schema validation",
            language: "YAML",
            distribution: LspDistribution::Npm {
                version: "1.15.0",
                package: "yaml-language-server@1.15.0",
                cmd: "yaml-language-server",
                args: &["--stdio"],
                node_required: None,
            },
        }),
        "pyright" => Some(LspServerMeta {
            id: "pyright",
            name: "Pyright",
            description: "Fast type checker and language server for Python",
            language: "Python",
            distribution: LspDistribution::Npm {
                version: "1.1.398",
                package: "pyright@1.1.398",
                cmd: "pyright-langserver",
                args: &["--stdio"],
                node_required: None,
            },
        }),
        "rust_analyzer" => Some(LspServerMeta {
            id: "rust_analyzer",
            name: "rust-analyzer",
            description: "Language server for the Rust programming language",
            language: "Rust",
            distribution: LspDistribution::Binary {
                version: "2025-03-17",
                cmd: "rust-analyzer",
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2025-03-17/rust-analyzer-aarch64-apple-darwin.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2025-03-17/rust-analyzer-x86_64-apple-darwin.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2025-03-17/rust-analyzer-aarch64-unknown-linux-gnu.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2025-03-17/rust-analyzer-x86_64-unknown-linux-gnu.gz",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2025-03-17/rust-analyzer-x86_64-pc-windows-msvc.gz",
                    },
                ],
            },
        }),
        "gopls" => Some(LspServerMeta {
            id: "gopls",
            name: "gopls",
            description: "Official Go language server maintained by the Go team",
            language: "Go",
            distribution: LspDistribution::Binary {
                version: "0.18.1",
                cmd: "gopls",
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://dl.google.com/go/gopls/v0.18.1/gopls.darwin-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://dl.google.com/go/gopls/v0.18.1/gopls.darwin-amd64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://dl.google.com/go/gopls/v0.18.1/gopls.linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://dl.google.com/go/gopls/v0.18.1/gopls.linux-amd64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://dl.google.com/go/gopls/v0.18.1/gopls.windows-amd64.zip",
                    },
                ],
            },
        }),
        "clangd" => Some(LspServerMeta {
            id: "clangd",
            name: "clangd",
            description: "Language server for C/C++ from the LLVM project",
            language: "C/C++",
            distribution: LspDistribution::Binary {
                version: "19.1.2",
                cmd: "clangd",
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/clangd/clangd/releases/download/19.1.2/clangd-mac-19.1.2.zip",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/clangd/clangd/releases/download/19.1.2/clangd-mac-19.1.2.zip",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/clangd/clangd/releases/download/19.1.2/clangd-linux-19.1.2.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/clangd/clangd/releases/download/19.1.2/clangd-windows-19.1.2.zip",
                    },
                ],
            },
        }),
        "lua_language_server" => Some(LspServerMeta {
            id: "lua_language_server",
            name: "lua-language-server",
            description: "Language server for Lua",
            language: "Lua",
            distribution: LspDistribution::Binary {
                version: "3.13.5",
                cmd: "lua-language-server",
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/LuaLS/lua-language-server/releases/download/3.13.5/lua-language-server-3.13.5-darwin-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/LuaLS/lua-language-server/releases/download/3.13.5/lua-language-server-3.13.5-darwin-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/LuaLS/lua-language-server/releases/download/3.13.5/lua-language-server-3.13.5-linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/LuaLS/lua-language-server/releases/download/3.13.5/lua-language-server-3.13.5-linux-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/LuaLS/lua-language-server/releases/download/3.13.5/lua-language-server-3.13.5-win32-x64.zip",
                    },
                ],
            },
        }),
        "taplo" => Some(LspServerMeta {
            id: "taplo",
            name: "Taplo",
            description: "TOML language server and toolkit",
            language: "TOML",
            distribution: LspDistribution::CargoInstall {
                version: "0.9.3",
                crate_name: "taplo-cli",
                cmd: "taplo",
                features: &["lsp"],
            },
        }),
        _ => None,
    }
}
