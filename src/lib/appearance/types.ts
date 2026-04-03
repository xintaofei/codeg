export type AccentColor =
  | "blue"
  | "purple"
  | "pink"
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "cyan"
  | "graphite"

export type InterfaceDensity = "compact" | "default" | "spacious"

export type TerminalScheme =
  | "default"
  | "solarized-dark"
  | "solarized-light"
  | "dracula"
  | "one-dark"
  | "nord"
  | "monokai"
  | "github-dark"
  | "github-light"

export type CodeThemeLight = "github-light" | "vitesse-light" | "min-light"

export type CodeThemeDark =
  | "github-dark"
  | "vitesse-dark"
  | "one-dark-pro"
  | "dracula"

export interface AppearanceSettings {
  accentColor: AccentColor
  uiFont: string
  codeFont: string
  uiFontSize: number
  codeFontSize: number
  codeThemeLight: CodeThemeLight
  codeThemeDark: CodeThemeDark
  terminalScheme: TerminalScheme
  density: InterfaceDensity
  reduceMotion: "system" | "on" | "off"
}
