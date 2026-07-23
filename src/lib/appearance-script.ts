// src/lib/appearance-script.ts

/**
 * Storage keys for appearance preferences.
 * 与 Provider 共享，确保 inline 脚本和 React 层读写同一份数据。
 */
export const STORAGE_KEY_THEME_COLOR = "codeg-theme-color"
export const STORAGE_KEY_ZOOM_LEVEL = "codeg-zoom-level"

// 新会话欢迎页「模式选择区域」（代码开发 / 日常办公 快捷卡片）是否显示。
// 缺省即回退为开启（保持历史行为）；仅在欢迎态客户端渲染，无需预水合。
export const STORAGE_KEY_WELCOME_QUICK_ACTIONS = "codeg-welcome-quick-actions"

// 字体偏好（界面 / 编辑器 / 终端）。
// 只有界面字体需要 *_STACK（已解析的 CSS font-family 栈），供 inline 脚本零依赖地
// 预水合写入 --font-sans；编辑器/终端字体只走各自的 Monaco/xterm 选项，水合后才挂载，
// 无需预水合，也不写任何全局 CSS 变量。*_FONT 存 id、*_CUSTOM 存自定义族名供回显。
export const STORAGE_KEY_UI_FONT = "codeg-ui-font"
export const STORAGE_KEY_UI_FONT_CUSTOM = "codeg-ui-font-custom"
export const STORAGE_KEY_UI_FONT_STACK = "codeg-ui-font-stack"
export const STORAGE_KEY_EDITOR_FONT = "codeg-editor-font"
export const STORAGE_KEY_EDITOR_FONT_CUSTOM = "codeg-editor-font-custom"
export const STORAGE_KEY_EDITOR_FONT_SIZE = "codeg-editor-font-size"
export const STORAGE_KEY_EDITOR_LIGATURES = "codeg-editor-ligatures"
export const STORAGE_KEY_EDITOR_WORD_WRAP = "codeg-editor-word-wrap"
export const STORAGE_KEY_TERMINAL_FONT = "codeg-terminal-font"
export const STORAGE_KEY_TERMINAL_FONT_CUSTOM = "codeg-terminal-font-custom"
export const STORAGE_KEY_TERMINAL_FONT_SIZE = "codeg-terminal-font-size"
export const STORAGE_KEY_TERMINAL_LIGATURES = "codeg-terminal-ligatures"

// Workspace 背景图片。图片本身存磁盘（~/.codeg/backgrounds/），localStorage 只存
// 展示配置。仅 enabled 与 panel-opacity 需要预水合（它们作用于首帧就存在的结构性
// 表面）；mask/blur/fill 水合后才有意义（图片异步到达），不进 inline 脚本。
export const STORAGE_KEY_WORKSPACE_BG_ENABLED = "codeg-workspace-bg-enabled"
export const STORAGE_KEY_WORKSPACE_BG_MASK = "codeg-workspace-bg-mask"
export const STORAGE_KEY_WORKSPACE_BG_BLUR = "codeg-workspace-bg-blur"
export const STORAGE_KEY_WORKSPACE_BG_FILL = "codeg-workspace-bg-fill"
export const STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY =
  "codeg-workspace-bg-panel-opacity"
// 图片存磁盘、写盘无跨窗口信号（外观设置是独立窗口）。用这个版本戳广播失效：
// 写/换/删图后 bump，让 workspace 窗口经 storage 事件重新读盘。不需预水合。
export const STORAGE_KEY_WORKSPACE_BG_IMAGE_VERSION =
  "codeg-workspace-bg-image-version"

/**
 * 同步执行的 inline 脚本，由 layout.tsx 通过 dangerouslySetInnerHTML 注入。
 *
 * 必须在第一帧渲染前完成 <html> 的 data-theme 属性和 font-size 内联样式写入，
 * 否则会出现 FOUC（先看到默认主题/字号，然后切换到用户偏好的闪烁）。
 *
 * 实现要点：
 * 1. 纯字符串，不依赖任何模块导入或外部符号 —— 避免 Next.js 把它当模块编译
 * 2. 白名单校验 —— localStorage 里的值若被篡改或残留旧版本，回退到默认
 * 3. try/catch 包裹 —— 隐私模式 / 嵌入 WebView 禁用 storage 时不抛错
 * 4. 数字常量与 theme-presets.ts 保持一致 —— 任何修改必须两边同步
 */
const SCRIPT = `
(function() {
  try {
    var VALID_COLORS = ["neutral","zinc","slate","stone","gray","red","rose","orange","green","blue","yellow","violet"];
    var VALID_ZOOMS = [80, 90, 100, 110, 125, 150];

    var storedColor = localStorage.getItem("${STORAGE_KEY_THEME_COLOR}");
    var color = VALID_COLORS.indexOf(storedColor) >= 0 ? storedColor : "neutral";
    document.documentElement.setAttribute("data-theme", color);

    var storedZoom = parseInt(localStorage.getItem("${STORAGE_KEY_ZOOM_LEVEL}") || "", 10);
    var zoom = VALID_ZOOMS.indexOf(storedZoom) >= 0 ? storedZoom : 100;
    document.documentElement.style.fontSize = (16 * zoom / 100) + "px";

    // 界面字体：预水合写入 --font-sans（普通组件与会话消息区都跟随它）。
    // stack 只是「显式选择」的缓存，不是偏好本身：仅当存在显式 id（codeg-ui-font）
    // 时才应用它。无显式选择的用户（含从旧默认升级、Provider 仅缓存过 stack 的用户）
    // 跳过，落到 :root 的 --font-sans 兜底（= 当前默认界面字体 Inter 栈），避免升级首屏闪字。
    // 无需在脚本里复制字体目录；空/超长/含越界字符同样跳过走默认。
    var uiFontId = localStorage.getItem("${STORAGE_KEY_UI_FONT}");
    var uiFontStack = localStorage.getItem("${STORAGE_KEY_UI_FONT_STACK}");
    if (uiFontId && uiFontStack && uiFontStack.length < 512 && !/[;{}<>]/.test(uiFontStack)) {
      document.documentElement.style.setProperty("--font-sans", uiFontStack);
    }

    // Workspace 背景：预水合仅处理首帧就存在的结构性表面。启用时给 <html> 打
    // data-workspace-bg 属性并预置 --ws-surface-alpha，避免面板 opaque→translucent
    // 跳变。图片本身异步从磁盘读，不在此处理。
    var wsbgEnabled = localStorage.getItem("${STORAGE_KEY_WORKSPACE_BG_ENABLED}");
    if (wsbgEnabled === "1") {
      document.documentElement.setAttribute("data-workspace-bg", "on");
      var wsbgAlpha = parseFloat(localStorage.getItem("${STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY}") || "");
      if (!isNaN(wsbgAlpha) && wsbgAlpha >= 0.3 && wsbgAlpha <= 1) {
        document.documentElement.style.setProperty("--ws-surface-alpha", String(wsbgAlpha));
      }
    }

    // 在 next-themes 水合之前同步检测暗色模式，防止白色闪屏。
    // next-themes 使用 localStorage key "theme"，attribute="class"。
    var storedMode = localStorage.getItem("theme");
    var isDark = storedMode === "dark" ||
        (storedMode !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
    if (isDark) {
      document.documentElement.classList.add("dark");
      document.documentElement.style.colorScheme = "dark";
      // 直接设置背景色，比等待 CSS 类匹配更快，覆盖"系统浅色 + 应用深色"场景
      document.documentElement.style.backgroundColor = "#09090b";
    } else {
      document.documentElement.style.colorScheme = "light";
      document.documentElement.style.backgroundColor = "";
    }
  } catch (e) {
    // localStorage 不可用时静默走默认
  }
})();
`

export const APPEARANCE_INIT_SCRIPT = SCRIPT
