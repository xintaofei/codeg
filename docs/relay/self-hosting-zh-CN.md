# Codeg Relay 自托管部署

Codeg Relay 是一个只转发端到端加密帧的 WebSocket 路由器。手机和电脑都
主动连接 Relay，因此电脑不需要公网 IP、路由器端口映射或直接暴露 Codeg
3080 端口。Relay 能看到 IP、路由标识、连接时间和密文大小，但无法解密聊天、
代码、命令、附件或 Codeg Token。

## 推荐架构

```text
Codeg Mobile --WSS--> relay.example.com --WSS--> Codeg Desktop
                         |
                    Nginx/Caddy TLS
                         |
                   127.0.0.1:18787
                         |
                    Codeg Relay
```

个人使用可从 1 vCPU、512 MiB 内存的 VPS 起步。文本任务的资源消耗很低，
附件流量是主要成本。公网只暴露 443；Relay 明文监听端口只绑定回环地址。

## 1. 准备域名和服务器

1. 准备一台安装了 Docker、Docker Compose 与 Nginx 或 Caddy 的 Linux VPS。
2. 将 `relay.example.com` 的 A/AAAA 记录指向 VPS。
3. 在防火墙中只开放 SSH、80 和 443，不开放 8787 或 18787。
4. 为域名申请受系统信任的 TLS 证书。手机端不接受公网 `ws://`。

## 2. 启动 Relay

在仓库根目录执行：

```bash
cd relay/deploy
cp relay.env.example relay.env
openssl rand -hex 32
```

把生成的随机值写入 `relay.env`：

```dotenv
CODEG_RELAY_DESKTOP_TOKENS={"*":"这里替换为刚生成的随机值"}
```

`"*"` 适用于只有一个信任域的私人 Relay，因为 Codeg 会自行生成桌面 ID。
面向互不信任的多个用户时，不应共享通配 Token，应为每个明确的
`desktop_id` 单独配置随机 Token。

启动服务：

```bash
docker compose up -d --build
docker compose ps
curl --fail http://127.0.0.1:18787/health
```

凭据哈希保存在 `relay/deploy/data/device-credentials.json`。只备份此数据目录，
不要把 `relay.env`、桌面 Token 或 FRP Token 提交到 Git。

## 3. 配置 HTTPS/WSS

### Nginx

在 `relay.example.com` 的 TLS `server` 块中引入
[`relay/deploy/nginx-frp.conf`](../../relay/deploy/nginx-frp.conf)，然后验证并重载：

```bash
sudo nginx -t
sudo systemctl reload nginx
curl --fail https://relay.example.com/health
```

配置会公开 `/health`、`/v1/ws`、`/v1/devices*` 和 `/v1/pairings*`，但不会
公开 `/metrics`。WebSocket 必须保留 `Upgrade` 和 `Connection` 请求头，并关闭
代理缓冲。

### Caddy

把 [`relay/deploy/Caddyfile.example`](../../relay/deploy/Caddyfile.example)
中的域名改成自己的域名，合并到 Caddyfile 后重载。Caddy 会自动申请和续期证书。

## 4. 连接 Codeg Desktop

1. 打开 **设置 → Web Service → 手机 Relay 访问**。
2. Relay WebSocket 地址填写 `wss://relay.example.com/v1/ws`。
3. 桌面 Relay Token 填写 `relay.env` 中同一个 32 字节以上随机值。
4. 开启移动访问并点击 **保存并应用**。
5. 确认状态显示桥接进程运行中，再生成手机配对二维码。
6. 手机选择 **Relay**，扫描二维码，并核对两端六位安全码后确认。

Relay 地址由电脑写入一次性二维码，APK 不绑定任何固定域名。切换域名或 Relay
运营方后，应在新 Relay 上重新配对，并在旧 Relay/电脑设置中撤销旧设备。

## 5. 可选：Relay 也在内网时使用 FRP

如果 Relay 进程本身不能运行在 VPS，可使用
[`relay/deploy/frpc.toml.example`](../../relay/deploy/frpc.toml.example) 把内网
8787 转发到 VPS 的 18787，再由 VPS 上的 Nginx/Caddy 提供 TLS。

这与 Direct 模式映射 Codeg 3080 不同：FRP 只承载 Relay 密文路由器，不应把
Codeg 3080 暴露到公网。必须让 frps 的 `proxyBindAddr` 为 `127.0.0.1`，或用
防火墙阻止公网直接访问 18787。

Relay 直接运行在 VPS 时不需要 FRP，这是更简单、更可靠的部署方式。

## 6. 验证

健康检查：

```bash
curl --fail https://relay.example.com/health
```

预期返回：

```json
{"protocol":1,"status":"ok"}
```

检查 HTTP 是否跳转到 HTTPS、证书域名是否匹配，并确认以下地址不能匿名读取设备：

```bash
curl -i https://relay.example.com/v1/devices?desktop_id=d_test
```

它应返回未授权错误。完成配对后，在手机切换 Wi-Fi/移动网络，确认能在五秒目标内
自动重连。

## 7. 安全清单

- 公网配置只允许 `wss://`，`ws://` 仅用于本机开发。
- 不公开 8787、18787 和 `/metrics`。
- 桌面 Token 使用至少 32 个随机字节，不与 FRP、Codeg 或其他服务共用。
- 二维码只在电脑前展示，核对手机与电脑的六位安全码。
- 定期更新 Docker 基础镜像、Nginx/Caddy 和 FRP。
- 为 IP、连接数和流量设置限速；公共服务不要使用通配 Token。
- Token、域名或服务器迁移后重新配对并撤销旧设备。
- Relay 只能保护传输；手机或电脑本身被入侵时，攻击者仍可能读取端侧明文。

## 8. 故障排查

### 健康检查正常，但 WebSocket 连不上

检查 `/v1/ws` 是否传递 WebSocket Upgrade 请求头、代理读取超时是否大于心跳间隔，
以及 CDN 是否允许 WebSocket。

### 桌面显示桥接运行中，但手机离线

“运行中”表示桥接任务正在重试，不一定代表 Relay 已完成鉴权。检查 Relay 日志中的
`Relay bridge connected`、Token 是否一致，以及手机二维码中的域名是否为当前域名。

### 换域名后旧手机仍连接旧地址

Relay URL 保存在手机安全配对配置中。用新域名重新生成二维码并配对，确认新连接后
撤销旧设备；不要只修改 DNS 后长期依赖旧凭据。
