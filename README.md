# TV-Rust
本项目基于`https://github.com/lushunming/TV-Multiplatform` 改造为ui为rust，爬虫为java的tvbox 电脑端。

## spider-host HTTP API

爬虫宿主 `spider-host` 通过本地 HTTP 提供业务能力（默认 `http://127.0.0.1:9980`，端口被占用自动顺延，实际端口见启动握手行 `hello`）：

- Swagger UI：`http://127.0.0.1:9980/swagger`（随 fat jar 内置，离线可用）
- OpenAPI：`http://127.0.0.1:9980/openapi.json`
- 通用 RPC：`POST /api/rpc`；语义化端点：`/api/config/load`、`/api/sites/{siteKey}/home|search|detail|player` 等
- 媒体代理仍在独立端口（默认 `9978`）的 `/proxy`

完整接口、参数与调用示例见 [`spider-host/docs/spider-host-api.md`](spider-host/docs/spider-host-api.md)。
