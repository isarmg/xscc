XSCC 控制端 beta

用途：从这台电脑连接已授权的被控电脑。支持 Windows 11 x86_64。

开始使用
1. 运行控制端 setup.exe，安装后从开始菜单打开 XSCC Controller Beta；
   也可解压控制端 portable.zip 后运行 xscc-controller.exe。
2. 在 xscs 创建 controller（控制端）实例，取得该实例的“密码”。
3. 在应用中输入 xscs 的 HTTPS 服务端地址和实例密码，完成绑定。
4. 在 xscs 授予此控制端访问对应 controlled 实例的权限。
5. 等待被控端在线，从授权列表选择目标并连接。

控制端按当前用户安装，无需注册主机服务。卸载前关闭应用；
卸载保留绑定身份与配置。如需解除绑定，请先在应用中执行解绑。
实际硬件能力由机器决定，不承诺零拷贝。

本包未签名，Windows 可能显示未知发布者提示。
Windows 11 真机联调尚未完成，具体验证结果以对应 Release 说明为准。

构建来源见 build-info.json；校验文件为包外带角色前缀的 *-SHA256SUMS.txt。
更多说明见对应源码版本的 docs/gpui-windows-beta.md。
