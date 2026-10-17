# QUIC 串流实验说明

当前实现用于开发验证，默认关闭，不代表发布包已启用或已获得性能提升。

## 启用边界

- 根 crate 构建时显式加入 `experimental-quic`，保留相应平台与角色的原有 features。该 feature 同时启用 `managed` 与 `base/experimental-quic`。
- GPUI 实验应用在原有 `live-session` / `native-host` 配置上追加 `librustdesk/experimental-quic`；仅编译 `base` 的 feature 不会启用应用接线。
- 未选中的客户端会话继续现有连接路径；被明确选为 QUIC 的会话失败即报错，**不会自动回退 TCP**。
- 主机未设置下面的监听配置时不监听 QUIC。部分配置缺失、证书无效或不存在已有主机身份时拒绝启动该实验监听。

## 必备授权与信任

控制端必须从管理端授权主机列表选择 `managed:<设备ID>`，且目标地址和端口必须在该主机当前授权地址列表中。仅修改环境变量不能增加管理权限，`test-lan-direct` 也不能绕过此检查。

TLS 1.3 校验证书信任根及服务器名称；主机另用现有设备身份签名 TLS exporter，控制端核对显式配置的主机公钥。管理授权证明绑定这个具体连接，运行期间继续检查授权有效性。TLS 成功和管理授权成功均不代替原有桌面登录、密码/2FA及输入、剪贴板权限检查。

证书、私钥和主机公钥应通过已批准的运维流程提供。这里不生成、不安装凭据，不提供跳过校验开关；主机私钥不得复制到客户端。当前关闭 0-RTT 和 QUIC 地址迁移。

## 环境变量

设置在实际运行对应角色的进程环境中，修改后重启该进程。移动端目前没有对应的用户设置界面，不能把桌面环境变量配置视为移动端部署已经验证。

主机端三项必须同时提供：

| 名称 | 内容 |
| --- | --- |
| `XSCC_EXPERIMENTAL_QUIC_LISTEN` | 明确的监听 IP 与非零 UDP 端口，例如 `0.0.0.0:21119` 或 `[::]:21119`；不接受主机名 |
| `XSCC_EXPERIMENTAL_QUIC_CERT_DER` | 单个 DER 格式服务器证书文件路径 |
| `XSCC_EXPERIMENTAL_QUIC_KEY_DER` | 与证书匹配的 PKCS#8 DER 私钥文件路径，仅主机进程读取 |

主机仍遵守停止服务、仅出站和禁用监听等现有开关。监听器使用已有设备身份；TLS 证书的密钥与设备身份密钥是两套不同的密钥。

控制端五项：

| 名称 | 内容 |
| --- | --- |
| `XSCC_EXPERIMENTAL_QUIC_PEER` | 与选择项完全一致的 `managed:<设备ID>`；仅此会话启用 QUIC |
| `XSCC_EXPERIMENTAL_QUIC_ENDPOINT` | 授权列表中的 `主机名:端口`、`IPv4:端口` 或 `[IPv6]:端口`，必须明确端口；不是 HTTP URL |
| `XSCC_EXPERIMENTAL_QUIC_SERVER_NAME` | 必须匹配服务器证书 SAN 的 TLS 校验名称 |
| `XSCC_EXPERIMENTAL_QUIC_ROOT_CERT_DER` | 明确信任的 DER 根证书文件路径；使用预先核验的自签名服务器证书时可直接信任该证书 |
| `XSCC_EXPERIMENTAL_QUIC_HOST_KEY` | 已核验的 XSCC 主机设备公钥，32 字节的十六进制表示（64 个十六进制字符）；不是 TLS 公钥或界面显示的指纹摘要 |

已保存的该 endpoint 设备身份若与 `HOST_KEY` 不同，连接会失败，不覆盖旧身份。授权地址按规范化后的地址和端口匹配；不能用解析到同一 IP 的其他主机名替换。控制端当前仅尝试 DNS 返回的首个地址。

## 网络与媒体限制

- 监听端口是 UDP，例子中的 21119 不是默认值。管理端授权地址也须包含客户端实际使用的地址和端口；仅开放 TCP 端口无效。
- 本协议 ALPN 为 `xscc-quic-experimental/1`，不是 HTTP/3。[Caddy 普通 HTTP `reverse_proxy`](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy) 不能直接转发此媒体协议。应提供 UDP 可达路径；若采用额外四层代理，须另外验证身份绑定、源地址/IP 白名单及端口路由，当前不承诺代理部署已通过测试。
- 视频走 QUIC DATAGRAM，按协商 MTU 分片并限量重组；缺片、整帧序号跳跃或过期会进入等待关键帧恢复。尚无 FEC、应用层选择性重传或真实弱网调优结果。
- 控制、输入、剪贴板分别走可靠流。**音频目前仍走可靠控制流**，未完成独立实时音频通道。
- 当前单条可靠消息上限 64 KiB；超限或可靠发送队列满会报错/断线。视频发送队列满会丢弃本次新帧，不是无限缓存；接收端根据序号缺口恢复参考链。
- 重组和刷新定时值只是当前实现边界，不是端到端延迟承诺。连接迁移、自动穿透/中继及 QUIC 失败后的自动 TCP 回退未实现。

## 验证

在具备项目原生编译依赖的环境中，从仓库根目录运行：

```sh
cargo test --locked -p base --lib --features experimental-quic direct_transport::quic::tests
cargo test --locked -p base --lib --features experimental-quic direct_transport::quic_media::tests
cargo test --locked --lib --features experimental-quic management::quic
cargo test --locked --lib --features experimental-quic client::quic::tests
cargo test --locked --lib --features experimental-quic quic_host::tests
cargo test --locked --lib --features experimental-quic session_transport::experimental::tests
```

GPUI 构建使用相应角色的原生 features 与 `--no-default-features`；CI 中已有独立测试过滤器，不能用匹配到零项的结果代替通过。

截至 2026-10-17，本地已通过 `base` QUIC/分片测试；管理授权、控制端选择和会话传输模块也已在隔离探针中通过测试，包括消息往返、缺片/整帧丢失恢复、队列压力、接收限制和授权撤销。隔离探针不等于完整应用构建。完整应用测试在当前环境被 `glib-2.0.pc` 等原生依赖缺失阻断，CI 结果仍需确认。

发布前还需验证：双端真实桌面会话与权限撤销、错误证书/公钥/授权地址拒绝、UDP 阻断明确失败、多显示器切换、连续输入/音频/剪贴板、真实编解码参考链，以及 Windows/macOS/Linux/Android/iOS 的实际构建与运行。弱网测试应记录分阶段耗时、p50/p95/p99、卡顿时长和带宽；当前没有可对外宣称的延迟改善数据。

源码入口：`src/client/quic.rs`、`src/quic_host.rs`、`src/session_transport.rs`、`src/management/quic.rs`、`libs/base/src/direct_transport/quic.rs` 与 `quic_media.rs`。
