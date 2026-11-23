# Core 开发入口

开始工作前阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。插件契约、指南、参考与官方 SDK 的维护入口是 [shared/plugins/README.md](shared/plugins/README.md)。

新增或修改公共插件 API、决定是否引入版本前，必须阅读 [插件 API PR 提交指南](shared/plugins/guides/plugin-api-pr-guide.zh-CN.md)，尤其是第 2–5、8、11 节。AI 同样必须遵守其中的领域复用与升版限制、可定位且脱敏的统一日志、错误码全契约查重，以及 Rust、Go、Node、Java 四个 SDK 同 PR 支持要求；example 可以后补。

契约规定的已有字段和运行时行为以 [contract/v1/README.md](shared/plugins/contract/v1/README.md) 为准；历史升版先例不能替代本次的版本判断。未修改契约或 SDK 实现的纯文档任务只运行相关文档校验。
