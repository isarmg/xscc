# 确认 xscc 正常运行

完成安装向导后，后台服务会持续连接管理端并处理任务。日常查看不需要重新运行安装或配对。

以下为 Linux 示例；Windows 使用管理员 PowerShell 去掉 sudo，macOS 使用 `/usr/local/bin/xscc`。

```sh
sudo xscc service status --format json
sudo xscc pair status --format json
sudo xscc status --check --format json
```

这三项分别回答：服务是否运行、本机绑定是什么、业务检查是否通过。最后在 xscs 中读取一次 Sunshine 配置并确认结果。

## 查看近期活动

```sh
sudo xscc logs --tail 100
```

在管理台提交的任务可通过 `sudo xscc tasks list --format json` 查看，单项使用 `sudo xscc tasks show OPERATION_ID --format json`。把 OPERATION_ID 换成列表中的实际值。

任务结果会持久保存，重连后继续上报。unknown 表示效果未确认，先检查实际资源及对应任务记录，再决定后续操作。管理台的使用步骤见 [xscs 使用指南](https://github.com/isarmg/xscs/blob/main/docs/usage.md)。

修改设置见[配置指南](configuration.md)，定向检查见[故障排查](troubleshooting.md)。

管理台会在提交前显示协议字段错误，客户端仍独立执行同一校验：应用名称及命令长度使用 UTF-8 字节，配置文本使用字段定义的 Unicode 字符数上限。首尾空白、控制字符及配置文本的 `# = [ ]` 均须由操作者改正；客户端不会修剪或改写主机命令。`PatchConfig` 保留为按修订条件更新的 API 能力；当前 Web 配置编辑器只提交完整的 `SaveConfig`。
