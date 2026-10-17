# GUI 业务逻辑共用的 CLI 入口

这是独立 GUI Fork，不是 Pan 插件内嵌的 cc-switch，也不是原 SaladDay CLI 的补丁。
基线为 GUI 上游 `farion1231/cc-switch` 的 `f3b18df12007d0fd79fd8ad8d310880664015197`。
源码位置 `D:\project\cc-switch-pan`；原 GUI/CLI 仓库只读。

## 原因与选择

旧 CLI 的 OAuth 账号存储结构仅有 account_id/email/refresh_token/authenticated_at，保存时强制 version 1。
GUI version 2 还保存 id_token/chatgpt_account_id/token_updated_at_ms；这些是 GUI 判定账号身份和接管 Codex refresh token 代次的依据。
旧 CLI `auth default` 对整表反序列化再保存会丢弃所有账号的上述字段。用三个假账号已复现，不涉及真实切换。
只补 JSON 字段不能对齐 GUI 的身份校验、native auth 接管、并发及切换事务。
因此这里增加薄入口，直接调用 GUI ProviderService、CodexOAuthManager 和额度服务，不复制或另写认证算法。

## 数据与进程

沿用 GUI `~/.cc-switch` 数据目录，也读取 GUI Tauri Store 的可选 app_config_dir_override。
`CC_SWITCH_CONFIG_DIR` 可明确指定同一数据目录；不会自动复制、移动或修复现有账号。
version 1 或缺少身份的旧记录由 GUI 隔离为需要重新认证，CLI 不伪造身份或复用失效 refresh token。
已经丢失的字段不能通过新程序凭空恢复；应从可靠备份恢复或由用户在 GUI 重新认证。

新 Fork GUI 运行时，CLI 使用本机 Windows Named Pipe 将请求交给同一个 AppState，使用同一账号管理器，GUI 收到状态更新事件后刷新列表。
GUI 未运行时，控制台入口调用同一 GUI 业务代码。旧 GUI/CLI 仍在运行而没有新 pipe 时，写入和额度刷新拒绝执行，避免多进程持有旧凭据覆盖。
同一路径 GUI 只能运行一个实例；不要同时运行原 GUI 和新 Fork。独立 console 可执行文件只是另一入口，不是第二套数据库。
本轮没有自动安装、替换程序、修改 PATH 或执行真实账号切换。

## 命令

独立控制台程序：`pan-cc-switch.exe`。
GUI 程序也支持 `cc-switch.exe --cli <相同命令>`（脚本场景推荐控制台入口，stdout 为 JSON）。

```powershell
pan-cc-switch.exe --version
pan-cc-switch.exe --app codex provider list --json
pan-cc-switch.exe auth status --json
pan-cc-switch.exe --app codex provider quota <provider-id> --json
pan-cc-switch.exe --app codex provider switch <provider-id>
pan-cc-switch.exe auth default <account-id>
```

仅支持 codex/claude/gemini 和 Pan bridge 已有命令。ID 按精确 ID 使用，不支持隐式列表序号或名称猜测。
不提供删除/导入/批量修改/自动更新/登录接口；登录、重新认证和编辑账号继续由 GUI 完成。
CLI 列表不返回 settingsConfig、access/refresh/id_token；错误不回传服务原始凭据。
额度不支持时明确返回 not_available，不伪装成零。无需开启 TCP/HTTP 服务。

## 构建与验收

`scripts/build-pan-cli.ps1` 使用仓库外独立构建缓存和工具链；加 `-Test` 运行入口单测。
普通 GUI 构建仍按原仓库命令进行；GNU 打包时跳过原先仅适用于 MSVC 的 manifest 链接标志。
GUI 包装用的 dist placeholder 只用于 console 编译，不能当成可用 GUI 前端发布；发布 GUI 时必须执行原有前端构建。

真实切换由用户执行：先退出旧 GUI，启动已构建的新 GUI，读取同一账号和 provider，记录原 ID；在 Pan bridge 的 settings.json 手动把 executable 指向新 console。
先读列表/额度，再由用户切换一个测试账号，核对 GUI 与 Panel，再恢复原 ID。未知结果不自动重放。
没有新 GUI 运行时，旧 GUI 进程存在应拒绝写入；不能把这种拒绝当作账号失效。
