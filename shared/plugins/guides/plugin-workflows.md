# 插件宿主交互与联网 / Plugin workflows

## 中文

普通工具插件可以组合原生输入、只读预览、对比、输出、进度和 HTTP，业务数据仍只在自己的插件仓库。先声明需要的权限，再用 `host.supports` 检查方法；旧宿主不认识新权限时会拒绝安装，需要等待包含能力的桌面版发布。准确字段和限额见[契约](../contract/v1/README.md#workflow-host-methods)。这些方法属于 `litheFunctions` 0 内的可选扩展，没有升级协议版本。

Node 示例（`workspace` 是宿主返回的上下文，`host` 来自已注册函数的调用上下文）：

```js
const selection = await host.window.showQuickPick(workspace, {
  title: 'Choose an entry', items: [{id:'entry-1',label:'First entry'}],
});
if (!selection?.selectedIDs.length) return;
const output = await host.output.create(workspace, {name:'Request output'});
const progress = await host.progress.create(workspace, {title:'Loading',cancellable:true},
  event => { if (event.cancelled) controller.abort(); });
try {
  const response = await host.network.request(workspace, {
    url:'https://example.com/data',maximumResponseBytes:65536,
  }, {signal:controller.signal});
  await host.output.append(workspace, {channelID:output.channelID,text:`HTTP ${response.status}\n`});
  await host.output.show(workspace, {channelID:output.channelID});
} finally {
  await host.progress.end(workspace, {progressID:progress.id});
  // 保留输出给用户阅读，工作结束或不再需要时调用 output.dispose。
}
```

示例前先创建 `const controller = new AbortController()`。Rust/Go 的 `progress.create` 返回 `Subscription`，用 `id()` / `ID()` 作为 `progressID`，有界读取取消事件，结束时调用 `end` 或 `dispose`。Java 通过 Listener 接收事件，返回的 `Subscription.id()` 是进度 ID。所有 SDK 的 `end` 同时清除本地事件路由。

`showInputBox` 的 `password:true` 用于明确向用户索取一个秘密，不预填、不记录；保存应调用本插件的 `credentials.set`。HTTP 不自动读取浏览器 Cookie，也不共享其他插件或宿主业务的凭据。默认返回重定向而不跟随，插件检查状态后自行决定下一步。网络请求由桌面发出，远程后端需要访问远端网络时使用自己的网络实现。

状态项的 `commandID` 只能引用本插件清单声明的命令。资源预览按 Markdown 或 HTML 的扩展名选择对应方法；省略呈现方式时显示编辑器和预览。浏览器打开资源默认要求缓冲区已保存；仅在明确希望打开磁盘旧内容时传 `unsaved: "openSavedFile"`，宿主不会自动保存。

树先返回已加载条目和 `cursor`。`expand` 再加载某父节点的子项；`loadMore` 返回累积范围或补丁，并更新 cursor。`views.refresh(workspace,{instance:event.instance})` 使当前显示失效；刷新后使用稳定 item ID 保留选择和展开。不要把每页当成新的整棵树而丢掉先前条目。

## English

Combine native input, read-only previews, text diff, output, progress and HTTP
without moving tool-specific data into the host. Declare each permission and
check `host.supports` first. See the [contract](../contract/v1/README.md#workflow-host-methods)
for exact shapes, limits, ownership and errors. These are optional additions to
`litheFunctions` 0; older hosts can refuse packages using unknown permissions.

Quick pick returns stable IDs or null on dismissal; input distinguishes an
accepted empty string from cancellation. Password fields never prefill secrets.
Use the plugin's credentials service for storage. HTTP uses desktop proxy and
certificate policy, has no browser cookie jar, returns redirects without following,
and bounds time and bytes. It does not grant authentication sessions or downloads.

Create output/progress per task and release them when finished. The subscription
ID returned by progress creation is also its progress ID. SDK end methods dispose
the local route; disposal of the subscription also ends the native progress.
Use cancellation events to abort your own operation.

Status commands must be declared by the same plugin. Use the preview method matching the resource extension; omitted presentation shows the editor and preview. Opening a browser resource defaults to requiring saved contents. Pass `unsaved: "openSavedFile"` only to explicitly open the disk version; the host never saves automatically.

For lazy trees, return retained items and an opaque cursor. On expand/loadMore,
return the full accumulated range or patches and advance the cursor. A repeated
cursor or non-growing range stops automatic paging. Refresh only an instance
owned by your current session/context, and keep stable item IDs across refresh.
