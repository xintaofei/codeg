# Codeg Mobile 品牌规范

## 核心 Token（OKLch）

```css
:root {
  --bg: oklch(17.2% 0.017 248);
  --surface: oklch(21.7% 0.021 245);
  --fg: oklch(97.6% 0.006 240);
  --muted: oklch(70.8% 0.025 246);
  --border: oklch(31.8% 0.034 247);
  --accent: oklch(70.4% 0.144 254);
}
```

## 字体

- Display：Inter / SF Pro Display / PingFang SC / HarmonyOS Sans
- Body：Inter / SF Pro Text / PingFang SC / HarmonyOS Sans
- Mono：JetBrains Mono / SF Mono / Menlo

## 设计姿态

- 对话是默认工作面，任务状态是紧邻顶部的快速入口，而不是独立仪表盘。
- 深色表面仅用明度与细边框分层，避免玻璃、霓虹和装饰性渐变。
- 状态始终同时使用图标、文字与轮廓形状，不把颜色作为唯一信号。
- 普通执行步骤保持单行紧凑；日志只在用户主动展开时出现。
- 重要操作集中在拇指区；iOS 触控区域不小于 44pt，Android 不小于 48dp。

系统一句话：Codeg Mobile 是安静、精密、对话优先的远程 AI 编程任务指挥中心。
