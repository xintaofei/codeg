# Codeg Mobile Android 0.20.3 发布候选

状态：**候选包，尚未完成 Android 真机硬验收，不得标记为正式发布。**

## 包信息

| 项目 | 值 |
| --- | --- |
| 文件 | `codeg-mobile-0.20.3-pr1-universal.apk` |
| 本地位置 | `dist/pr-release/codeg-mobile-0.20.3-pr1-universal.apk`（构建产物，不进入 Git） |
| Application ID | `cn.crain.codeg.mobile` |
| Version name | `0.20.3` |
| Version code | `20003` |
| 最低 Android | API 24 / Android 7.0 |
| Target SDK | 36 |
| SHA-256 | `b52ea281cc8a335ea3bd9f7c0aef752acd6bcfb1b3b08fa59a65c6ac599aa00a` |
| ABI | `arm64-v8a`、`armeabi-v7a`、`x86`、`x86_64` |
| PR 公开预发布 | [`android-v0.20.3-pr1`](https://github.com/Crain99/codeg/releases/tag/android-v0.20.3-pr1) |

## 签名

- Release 密钥位于仓库外，口令保存在 macOS Keychain。
- RSA 4096 位。
- APK Signature Scheme v2 验证通过。
- 证书主题：`CN=Codeg Mobile, OU=Mobile, O=Crain99, L=Shanghai, ST=Shanghai, C=CN`。
- 证书 SHA-256：`94ccc1b08ec6af8a5825fc9acf1148b206d8a2c028633372f44d3328dabf0c05`。
- Release Manifest 不包含 `android:debuggable=true`。

## 已通过的候选包验证

- 前端：192 个测试文件、2348 个测试通过，ESLint 无错误，TypeScript 检查和 Next.js 生产构建通过。
- Relay Bridge：10 个 Rust 测试通过，包含真实 multipart 上传、700 KB 附件、幂等重试、取消和健康会话重连退避重置；严格 Clippy 通过。
- Relay 服务：9 个 Rust 测试通过；真实 WebSocket 验证两台移动设备并发路由互不串流，撤销会立即断开在线设备且旧凭据无法重新鉴权；严格 Clippy 通过。
- Android universal Release APK 构建成功，四种 ABI 均包含 Release 原生库。
- Android API 35 干净模拟器冷安装和冷启动成功，无崩溃。
- 使用同一 Release 签名覆盖安装成功，应用数据保留。
- Android 官方 `apksigner` 验证通过。
- 通过公网 Relay 上传 700 KB 附件，APK 显示 0% 到 100% 的分片进度，桌面落盘大小和 SHA-256 与手机源数据一致。
- 通过公网 Relay 取消 2 MB 附件上传，手机未附加文件且桌面无残留。
- 2 MB 上传到 10% 时强制终止桌面进程；桌面重启并重新握手后恢复到 100%，只产生一个大小和 SHA-256 均正确的文件。
- 受控重启公网 Relay 后，手机从 WebSocket 断开到收到首个加密帧用时 3.696 秒；从执行 Relay 重启开始计时为 4.900 秒，达到前台五秒恢复目标。
- Bridge 双向连续处理 2048 个有序加密帧，移动 RelayTransport 连续处理 512 个事件；重复帧被拒绝且事件只分发一次。
- 移动 Relay 断线后每 500 毫秒重试，回归测试验证不会继承较长退避；Android WebView 锁屏/唤醒实测触发 `hidden → visible`，页面和会话上下文仍存在。
- 对受控 Relay 最近四小时的日志执行业务明文模式审计，代码、附件名、聊天正文、Bearer/Codeg Token 等命中数为 0。

## 已知验收边界

- Android 模拟器的“飞行模式关闭命令 → 收到首个加密帧”压力测试为 6.362 秒，其中模拟器网络栈恢复占主要时间；这不是 Wi-Fi/移动网络切换的通过证据，正式发布仍必须在真实手机上验证五秒目标。
- 之前的本地 debug/候选包可能使用不同签名；遇到 Android“签名冲突”时需先卸载旧测试包并重新配对。本 PR 公布的 Release 证书作为后续 PR 候选包的固定升级签名。

## iOS 状态

项目保留 iOS 模拟器编译检查，但当前没有可用的 Apple Developer 分发证书和
Provisioning Profile，因此本 PR 暂不提供 IPA 或 TestFlight 包。

## 正式发布前仍需完成

- 在至少一台没有开发工具的 Android 真机安装此候选包。
- 真机完成一次性二维码配对、桌面安全码确认和真实 Agent 任务闭环。
- 真机验证 Wi-Fi/移动网络切换、锁屏返回和前台五秒恢复。
- 合并后由上游维护者决定正式 Android Release、签名托管和后续升级渠道。

安装与配对步骤见 [`android-install-zh-CN.md`](android-install-zh-CN.md)。
