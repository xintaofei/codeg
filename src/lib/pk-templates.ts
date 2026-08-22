/**
 * Built-in PK task templates — visually verifiable creative prompts that
 * are proven conversation starters in the AI community.
 *
 * Each template fills the launcher's task textarea in one click, lowering
 * the barrier to starting a match. Task text is in Chinese (the primary
 * user base); i18n only translates the short label shown on the chip.
 *
 * Sources are documented in docs/PK-ROADMAP.md (功能 1).
 */

export interface PkTaskTemplate {
  /** Stable id for the template. */
  id: string
  /** Short display label (i18n key under PkArena.templates.<id>). */
  labelKey: string
  /** Emoji shown on the template chip. */
  emoji: string
  /** The task text to fill into the textarea. */
  task: string
}

/** Built-in templates, ordered by visual impact / viral potential. */
export const PK_TEMPLATES: readonly PkTaskTemplate[] = [
  {
    id: "pelican",
    labelKey: "pelican",
    emoji: "🦤",
    task: "生成一幅鹈鹕骑自行车的 SVG 图。",
  },
  {
    id: "bouncingBall",
    labelKey: "bouncingBall",
    emoji: "⚽",
    task: "做一个 HTML 动画:一个小球从三角形中心出发,每次撞到边就加速,并且多边形增加一条边(三角形→正方形→五边形→六边形……)。",
  },
  {
    id: "jellyBlob",
    labelKey: "jellyBlob",
    emoji: "🫧",
    task: "做一个浏览器小玩具:一个果冻 blob,可以戳、抓、拉伸它。没有计分、没有关卡,就是一个手感很好的 blob。",
  },
  {
    id: "blackHole",
    labelKey: "blackHole",
    emoji: "🕳️",
    task: "做一个 HTML 黑洞效果:中心一个吸积盘环绕旋转,背景星空被引力透镜扭曲拉扯,附近的光线被拖入事件视界。用 Canvas 或 WebGL 实现,鼠标移动时可以改变观察视角。",
  },
  {
    id: "snake",
    labelKey: "snake",
    emoji: "🐍",
    task: "用单个 HTML 文件写一个贪吃蛇游戏,键盘控制。",
  },
  {
    id: "flappyBird",
    labelKey: "flappyBird",
    emoji: "🐤",
    task: "用单个 HTML 文件写一个 Flappy Bird 克隆,Canvas 渲染。",
  },
  {
    id: "voiceChat",
    labelKey: "voiceChat",
    emoji: "🎙️",
    task: "用 Web Speech API 做一个支持语音的聊天机器人网页应用。",
  },
] as const
