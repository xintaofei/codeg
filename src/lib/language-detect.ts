// Map file extensions to Monaco editor language IDs.
//
// All values must be valid IDs registered by monaco-editor's basic-languages
// contribution; unknown IDs are silently treated as plaintext by Monaco, which
// is misleading for callers — so we prefer "plaintext" over a wrong-but-close
// mapping (e.g. Groovy is NOT mapped to "java", Zig is NOT mapped to "rust").
//
// .toml is mapped to "ini" as a best-effort approximation; TOML supports
// nested tables and typed values that ini grammar does not understand, but
// partial highlighting is preferable to plain text for this widely-used format.
const EXTENSION_MAP: Record<string, string> = {
  // TypeScript / JavaScript
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascript",

  // Systems
  rs: "rust",
  go: "go",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  "c++": "cpp",
  hh: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  cs: "csharp",
  fs: "fsharp",
  fsx: "fsharp",
  fsi: "fsharp",
  vb: "vb",

  // JVM
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  scala: "scala",
  sc: "scala",
  clj: "clojure",
  cljs: "clojure",
  cljc: "clojure",

  // Scripting
  py: "python",
  pyw: "python",
  pyi: "python",
  rb: "ruby",
  rake: "ruby",
  php: "php",
  pl: "perl",
  pm: "perl",
  lua: "lua",
  r: "r",
  jl: "julia",
  dart: "dart",
  swift: "swift",
  m: "objective-c",
  mm: "objective-c",
  ex: "elixir",
  exs: "elixir",

  // Shell
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  fish: "shell",
  ps1: "powershell",
  psm1: "powershell",
  psd1: "powershell",
  bat: "bat",
  cmd: "bat",

  // Data / config
  json: "json",
  jsonc: "json",
  json5: "json",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  conf: "ini",
  cfg: "ini",
  env: "ini",
  properties: "ini",
  xml: "xml",
  xsd: "xml",
  xsl: "xml",
  plist: "xml",
  svg: "xml",
  proto: "proto",
  graphql: "graphql",
  gql: "graphql",

  // Markup
  md: "markdown",
  markdown: "markdown",
  mdx: "mdx",
  rst: "restructuredtext",

  // Web
  html: "html",
  htm: "html",
  vue: "html",
  svelte: "html",
  hbs: "handlebars",
  handlebars: "handlebars",
  twig: "twig",
  pug: "pug",
  jade: "pug",
  liquid: "liquid",
  razor: "razor",
  cshtml: "razor",
  css: "css",
  scss: "scss",
  sass: "scss",
  less: "less",

  // Database
  sql: "sql",
  pgsql: "pgsql",
  mysql: "mysql",
  redis: "redis",

  // Misc
  dockerfile: "dockerfile",
  sol: "sol",
  tf: "hcl",
  tfvars: "hcl",
  hcl: "hcl",
  cypher: "cypher",
  cql: "cypher",
  wgsl: "wgsl",
  abap: "abap",
  apex: "apex",
  bicep: "bicep",
}

// Filenames without a meaningful extension (or whose full basename carries the
// language signal) — e.g. Dockerfile, Gemfile, .bashrc.
const BASENAME_MAP: Record<string, string> = {
  // Container
  dockerfile: "dockerfile",
  containerfile: "dockerfile",

  // Ruby tooling conventions
  gemfile: "ruby",
  rakefile: "ruby",
  podfile: "ruby",
  brewfile: "ruby",
  vagrantfile: "ruby",

  // Shell rc/profile dotfiles
  ".bashrc": "shell",
  ".bash_profile": "shell",
  ".bash_login": "shell",
  ".bash_logout": "shell",
  ".zshrc": "shell",
  ".zshenv": "shell",
  ".zprofile": "shell",
  ".zlogin": "shell",
  ".zlogout": "shell",
  ".profile": "shell",
  ".inputrc": "shell",
}

// HTML documents we can render in the in-app sandboxed preview. Scoped to real
// .html/.htm files — .vue/.svelte also map to the "html" language but are not
// standalone, renderable documents.
export function isHtmlPreviewable(path: string | null | undefined): boolean {
  if (!path) return false
  const basename = path.toLowerCase().split(/[\\/]/).pop() ?? ""
  const dot = basename.lastIndexOf(".")
  if (dot === -1) return false
  const ext = basename.slice(dot + 1)
  return ext === "html" || ext === "htm"
}

export function languageFromPath(path: string): string {
  const lower = path.toLowerCase()
  const basename = lower.split(/[\\/]/).pop() ?? lower

  if (BASENAME_MAP[basename]) {
    return BASENAME_MAP[basename]
  }

  // Dockerfile.dev / Dockerfile.prod / Dockerfile.test — common multi-stage
  // naming where the suffix is the build target rather than a file extension.
  if (basename.startsWith("dockerfile.")) {
    return "dockerfile"
  }

  const dotIdx = basename.lastIndexOf(".")
  if (dotIdx === -1 || dotIdx === basename.length - 1) {
    return "plaintext"
  }
  const ext = basename.slice(dotIdx + 1)
  return EXTENSION_MAP[ext] ?? "plaintext"
}
