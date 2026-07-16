# Codeg Mobile Android 0.20.3 发布候选

状态：**候选包，尚未完成 Android 真机硬验收，不得标记为正式发布。**

## 包信息

| 项目 | 值 |
| --- | --- |
| 文件 | `codeg-mobile-0.20.3-universal.apk` |
| 本地位置 | `dist/android/codeg-mobile-0.20.3-universal.apk`（构建产物，不进入 Git） |
| Application ID | `cn.crain.codeg.mobile` |
| Version name | `0.20.3` |
| Version code | `20003` |
| 最低 Android | API 24 / Android 7.0 |
| Target SDK | 36 |
| SHA-256 | `eee9efd1b1481500f278fb0ebef223f8db06c64123810df85ababd50ee06c4a0` |
| ABI | `arm64-v8a`、`armeabi-v7a`、`x86`、`x86_64` |

## 签名

- Release 密钥位于仓库外，口令保存在 macOS Keychain。
- RSA 4096 位。
- APK Signature Scheme v2 验证通过。
- 证书主题：`CN=Crain Codeg Mobile, OU=Mobile, O=Crain99, L=Shanghai, ST=Shanghai, C=CN`。
- 证书 SHA-256：`31619bbbd7be66ab8581ff52a8022c0e489147f4a4bf42fa6c846354d0a1c9ce`。
- Release Manifest 不包含 `android:debuggable=true`。

## 已通过的候选包验证

- 前端：190 个测试文件、2341 个测试通过，ESLint 无错误，Next.js 生产构建通过。
- Relay Bridge：9 个 Rust 测试通过，包含真实 multipart 上传、700 KB 附件、幂等重试和取消；严格 Clippy 通过。
- Android universal Release APK 构建成功，四种 ABI 均包含 Release 原生库。
- Android API 35 干净模拟器冷安装和冷启动成功，无崩溃。
- 使用同一 Release 签名覆盖安装成功，应用数据保留。
- Android 官方 `apksigner` 验证通过。
- 通过公网 Relay 上传 700 KB 附件，APK 显示 0% 到 100% 的分片进度，桌面落盘大小和 SHA-256 与手机源数据一致。
- 通过公网 Relay 取消 2 MB 附件上传，手机未附加文件且桌面无残留。
- 2 MB 上传到 10% 时强制终止桌面进程；桌面重启并重新握手后恢复到 100%，只产生一个大小和 SHA-256 均正确的文件。

## 正式发布前仍需完成

- 在至少一台没有开发工具的 Android 真机安装此候选包。
- 真机完成一次性二维码配对、桌面安全码确认和真实 Agent 任务闭环。
- 真机验证 Wi-Fi/移动网络切换、锁屏返回和前台五秒恢复。
- 验证设备撤销后旧凭据无法重连，并检查 Relay 日志无业务明文。
- 完成上述项目后创建私有 GitHub Release，并将本文件状态改为正式发布。

安装与配对步骤见 [`android-install-zh-CN.md`](android-install-zh-CN.md)。
