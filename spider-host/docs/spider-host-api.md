# spider-host 调用 API 文档

> 版本：spider-host **v1.2.0**（API 层已从 stdio JSON 行协议迁移为本地 HTTP）
> 依据源码：`com/corner/host/Main.kt`、`HttpApiServer.kt`、`Host.kt`、`JarLoader.kt`、`ProxyServer.kt`
> 调用方参考：`tv-rust/src/core/host.rs`、`tv-rust/src/app.rs`

---

## 0. 架构总览

`spider-host` 是独立的 JVM 子进程，由 Rust UI 以 `java -jar spider-host-all.jar` 拉起。对外有三条通道：

| 通道 | 方向 | 用途 |
|---|---|---|
| **stdout 握手** | 宿主 → Rust stdout | 启动后输出一行 `hello`，下发 API / 代理实际端口 |
| **本地 HTTP API** | Rust / 浏览器 / curl → `http://127.0.0.1:<apiPort>` | 所有业务调用（配置、首页、分类、搜索、详情、播放）与 Swagger 调试 |
| **本地 HTTP `/proxy`** | 爬虫/播放器 → `http://127.0.0.1:<proxyPort>` | 图片、字幕、m3u8 等媒体转发（爬虫 jar 内硬编码 9978） |

关键实现事实：

- 入口 `com.corner.host.MainKt`；业务 API 只监听 **127.0.0.1**。
- API 端口：默认 **9980–9999** 中第一个可用端口；全被占用时使用系统临时端口。实际端口写入 `hello.apiPort` / `hello.api`。
- 代理端口：仍由 `ProxyServer` 提供，默认 **9978–9999** 中第一个可用端口，写入 `hello.proxyPort` / `hello.proxy`。
- **stdin/stdout 不再承载业务请求**：stdout 只输出一行 hello；stdin 只用于 `quit` 或父进程退出（EOF）。
- `loadConfig` 是内容类调用的前置条件，且会清空已加载 jar / 爬虫实例缓存。
- 爬虫 jar 用 `URLClassLoader` 加载，宿主是父加载器（爬虫可直接使用 `Spider`、OkHttp、Gson、Jsoup、Hutool 等，见 `build.gradle.kts` 的 `api` 依赖）。
- 所有响应均为 JSON 信封，不使用请求 id；并发由 HTTP 与 NanoHTTPD 线程池自然隔离。

---

## 1. 启动与握手

### 1.1 启动

```bash
java -jar spider-host-all.jar
```

终端会看到一行 JSON（stdout），其余日志全部走 stderr：

```json
{"id":0,"ok":true,"method":"hello","result":"spider-host ready","version":"1.2.0","java":"21.0.1","apiPort":9980,"api":"http://127.0.0.1:9980","swagger":"http://127.0.0.1:9980/swagger","openapi":"http://127.0.0.1:9980/openapi.json","proxyPort":9978,"proxy":"http://127.0.0.1:9978"}
```

| 字段 | 说明 |
|---|---|
| `version` | 宿主版本，当前 `1.2.0` |
| `apiPort` / `api` | HTTP API 端口与根地址；`-1`/空串表示启动失败 |
| `swagger` / `openapi` | Swagger UI 与 OpenAPI 描述地址 |
| `proxyPort` / `proxy` | 媒体代理端口与根地址；`-1` 表示代理启动失败 |

> 若 API 端口启动失败（9980–9999 与临时端口均不可用），宿主会输出 hello 后立即退出，Rust 侧报“HTTP API 未就绪”。

### 1.2 生命周期

- `stdin` 收到 `quit`（纯文本或 `{"method":"quit"}`）→ 停止 API/代理并退出。
- `stdin` EOF（父进程退出）→ 同样退出，避免孤儿进程。
- `stdin` 收到其他内容 → 记录 WARN 后忽略（业务请求已迁移到 HTTP）。
- 也可以调用 `POST /api/shutdown` 主动停止（先回包，250ms 后退出）。

### 1.3 监听范围与安全

- 仅绑定回环地址，不对外网开放。
- **没有鉴权**：本机任意进程都可调用。请勿把端口转发到公网。
- 所有响应带 CORS 头，允许浏览器（Swagger UI / 本地调试页）直接访问。

---

## 2. Swagger / OpenAPI 调试

宿主内置 Swagger UI，离线可用（swagger-ui 资源以 webjar 打进 fat jar）：

| 地址 | 内容 |
|---|---|
| `http://127.0.0.1:9980/swagger` | Swagger UI（`/` 会 302 到这里） |
| `http://127.0.0.1:9980/openapi.json` | OpenAPI 3.0.3 描述 |
| `http://127.0.0.1:9980/v3/api-docs` | 同上（兼容别名） |
| `http://127.0.0.1:9980/api/health` | 健康检查，不依赖配置 |
| `http://127.0.0.1:9980/api/sites` | 已加载站点摘要 |

推荐调试顺序：

1. 启动宿主，浏览器打开 `/swagger`；
2. `POST /api/config/load` 填入配置 JSON（`data` 字段是**字符串**，不是对象）；
3. `GET /api/sites` 确认站点已加载；
4. 用 `POST /api/sites/{siteKey}/home`、`/search`、`/detail` 等逐个测试；
5. `POST /api/shutdown` 退出宿主。

> `/openapi.json` 的 `servers` 使用相对路径 `/`，Swagger 会按当前页面 origin 发请求，端口变化无需改文档。

---

## 3. 统一响应与状态码

### 3.1 响应信封

```json
{"ok":true,"result":{},"error":null,"elapsedMs":12}
```

| 字段 | 类型 | 说明 |
|---|---|---|
| `ok` | boolean | 业务是否成功 |
| `result` | any | 成功时的返回值，可能为 `null` |
| `error` | string\|null | 失败原因，格式 `ExceptionClass: message` |
| `elapsedMs` | number | 宿主处理耗时（毫秒），健康检查等轻量端点可能省略 |

> 不再有 `id` 字段，也无需按 id 匹配乱序响应。

### 3.2 HTTP 状态码

| 状态码 | 场景 |
|---|---|
| `200` | 调用完成（是否可用仍看 `ok`） |
| `400` | 请求体不是 JSON 对象、缺少必填字段、未知方法、站点不存在 |
| `404` | 路径不存在 / 静态资源缺失 |
| `500` | 爬虫初始化、jar 下载、解析等异常 |
| `503` | `/api/shutdown` 时未注册退出钩子（正常启动不会出现） |

`ok=false` 时 `result` 恒为 `null`；`ok=true` 时 `error` 恒为 `null`。**不要用 `result==null` 判断失败**。

请求体与响应体统一使用 **UTF-8 JSON**。宿主直接按 UTF-8 读取原始 body，不依赖 `Content-Type` 是否带 `charset`；中文搜索词、站点名、`extend` 值不会因客户端省略 charset 而乱码。

### 3.3 超时

超时由客户端负责。HTTP 客户端应设置请求超时；宿主不会主动回超时包。推荐值（与 Rust 实现一致）：

| 操作 | 超时 |
|---|---|
| `setRecent` / `destroy` | 5s |
| `loadConfig` / `homeContent` / `categoryContent` / `detailContent` | 60s |
| `searchContent` | 90s |
| `playerContent` | 45s |

Rust 的 `SpiderHost::request` 会把“等待 hello 下发 api 地址”的时间计入同一个超时。

---

## 4. 通用 RPC：`POST /api/rpc`

与宿主方法一一对应，Rust 客户端使用该入口；语义化端点只是它的便捷封装。

### 4.1 请求

```http
POST /api/rpc
Content-Type: application/json
```

```json
{"method":"homeContent","params":{"siteKey":"qq","filter":true}}
```

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `method` | string | 是 | 方法名，见下表 |
| `params` | object | 否 | 方法参数，缺省 `{}` |

### 4.2 方法一览

| method | 必填 params | `result` 形态 |
|---|---|---|
| `loadConfig` | `data` | 归一化配置 JSON 字符串 |
| `homeContent` | `siteKey` | JSON 字符串，或 `{data, warning}` |
| `homeVideoContent` | `siteKey` | 同上 |
| `categoryContent` | `siteKey`,`tid`,`pg` | 同上 |
| `detailContent` | `siteKey`,`ids` | 同上 |
| `searchContent` | `siteKey`,`key` | 同上 |
| `playerContent` | `siteKey`,`flag`,`id` | **纯 JSON 字符串**（不包 warning） |
| `setRecent` | `siteKey` | `null` |
| `serverInfo` | – | object |
| `setProxy` | `proxy` | `null` |
| `proxyLocal` | `siteKey`,`params` | array |
| `destroy` | `siteKey` | `null` |
| `clearCache` | – | `null` |

> “JSON 字符串”指 `result` 是 **string**，内部才是爬虫输出的 JSON —— 客户端需要**二次解析**（Rust 的 `request_spider_result` 即此行为）。

### 4.3 示例

```bash
curl -s -X POST http://127.0.0.1:9980/api/rpc \
  -H 'Content-Type: application/json' \
  -d '{"method":"serverInfo","params":{}}'
```

```json
{"ok":true,"result":{"version":"1.2.0","apiPort":9980,"apiBase":"http://127.0.0.1:9980","swagger":"http://127.0.0.1:9980/swagger","openapi":"http://127.0.0.1:9980/openapi.json","proxyPort":9978,"proxyBase":"http://127.0.0.1:9978","sites":12,"cfgId":"cfg-001"},"error":null,"elapsedMs":1}
```

---

## 5. 语义化端点

所有内容端点都是 `POST`，请求体为 JSON 对象；`siteKey` 在路径里。

| 方法 | 路径 | 请求体 | 对应宿主方法 |
|---|---|---|---|
| GET | `/api/health` | – | 健康检查 |
| GET | `/api/server/info` | – | `serverInfo` |
| GET | `/api/sites` | – | 站点摘要 |
| POST | `/api/rpc` | `{method,params}` | 通用分发 |
| POST | `/api/config/load` | `{data,url,cfgId}` | `loadConfig` |
| POST | `/api/proxy/set` | `{proxy}` | `setProxy` |
| POST | `/api/cache/clear` | – | `clearCache` |
| POST | `/api/shutdown` | – | 退出进程 |
| POST | `/api/sites/{siteKey}/home` | `{filter}` | `homeContent` |
| POST | `/api/sites/{siteKey}/home-video` | – | `homeVideoContent` |
| POST | `/api/sites/{siteKey}/category` | `{tid,pg,filter,extend}` | `categoryContent` |
| POST | `/api/sites/{siteKey}/search` | `{key,quick,pg}` | `searchContent` |
| POST | `/api/sites/{siteKey}/detail` | `{ids}` | `detailContent` |
| POST | `/api/sites/{siteKey}/player` | `{flag,id,vipFlags}` | `playerContent` |
| POST | `/api/sites/{siteKey}/recent` | – | `setRecent` |
| POST | `/api/sites/{siteKey}/destroy` | – | `destroy` |
| POST | `/api/sites/{siteKey}/proxy-local` | `{params}` | `proxyLocal` |

### 5.1 加载配置

```bash
curl -s -X POST http://127.0.0.1:9980/api/config/load \
  -H 'Content-Type: application/json' \
  -d '{"data":"{\"spider\":\"https://x.com/spider.jar\",\"sites\":[{\"key\":\"qq\",\"name\":\"腾讯\",\"type\":3,\"api\":\"csp_QQ\"}]}","url":"https://x.com/config.json","cfgId":"cfg-001"}'
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `data` | string | 是 | 配置源原始 JSON **文本**（不是对象） |
| `url` | string | 否 | 配置源地址，用于解析相对 jar/ext 路径 |
| `cfgId` | string | 否 | 配置标识，仅用于 `serverInfo` 回显 |

`result` 是归一化后的配置字符串：

```json
{
  "spider": "https://x.com/spider.jar",
  "sites": [
    {
      "key": "qq", "name": "腾讯", "type": 3, "api": "csp_QQ",
      "ext": "", "jar": "https://x.com/spider.jar",
      "categories": [], "header": {}
    }
  ]
}
```

归一化规则（`Host.loadConfig`）：

1. 只解析 `spider`、`sites` 两个顶层字段，其余字段忽略（`ignoreUnknownKeys`）。
2. `site.api`：`http`/`file` 原样保留；以 `.js` 结尾则按 `baseUrl` 转绝对地址；否则保留（如 `csp_XXX`）。
3. `site.ext`：`file` 前缀读取本地文件内容；以 `.js/.json/.txt` 结尾转绝对地址；否则原样。
4. `site.jar` 为空时回退到全局 `spider`。
5. 返回体是**完整站点表重新序列化**的结果。

副作用：清空 `JarLoader` 与爬虫实例缓存 → 下次业务调用会重新下载并 `init`。

### 5.2 站点列表

```bash
curl -s http://127.0.0.1:9980/api/sites
```

```json
{"ok":true,"result":[{"key":"qq","name":"腾讯","api":"csp_QQ","jar":"https://x.com/spider.jar","type":3}],"error":null}
```

未加载配置时 `result` 为 `[]`。

### 5.3 首页 / 分类 / 搜索 / 详情

```bash
# 首页（分类 + 筛选 + 推荐）
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/home \
  -H 'Content-Type: application/json' -d '{"filter":true}'

# 首页推荐
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/home-video \
  -H 'Content-Type: application/json' -d '{}'

# 分类
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/category \
  -H 'Content-Type: application/json' \
  -d '{"tid":"1","pg":"2","filter":true,"extend":{"area":"大陆","year":"2024"}}'

# 搜索
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/search \
  -H 'Content-Type: application/json' \
  -d '{"key":"繁花","quick":false,"pg":"1"}'

# 详情（ids 支持数组或逗号分隔字符串）
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/detail \
  -H 'Content-Type: application/json' -d '{"ids":["m123"]}'
```

参数细节：

| 端点 | 参数 | 说明 |
|---|---|---|
| `home` | `filter` | 是否请求筛选项；**必须是 JSON 布尔**，传 `1` 会被当 false |
| `category` | `tid` | 分类 id |
| | `pg` | 页码，字符串；空串会被替换为 `"1"` |
| | `extend` | `object<string,string>`，值会转字符串 |
| `search` | `key` | 关键词 |
| | `quick` | 快速搜索，默认 false |
| | `pg` | 页码，默认 `"1"` |
| `detail` | `ids` | 字符串数组；也接受逗号分隔字符串 |

### 5.4 播放

```bash
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/player \
  -H 'Content-Type: application/json' \
  -d '{"flag":"qq","id":"https://v.qq.com/x/cover/xxx.html","vipFlags":[]}'
```

- `flag`：线路名（来自 `vod_play_from`）。
- `id`：单集地址（来自 `vod_play_url` 解析结果）。
- `vipFlags`：解析器白名单数组，可空。
- 返回**不受 `warning` 包装**，直接是 JSON 字符串。

典型返回字段：`parse`、`jx`、`url`、`header`、`flag`、`playUrl`、`msg`。

### 5.5 最近站点 / 销毁 / 代理

```bash
# /proxy 回退到站点实例方法前建议调用
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/recent

# 调用站点爬虫 destroy()（不从缓存移除）
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/destroy

# 出站代理：支持 host:port / http://host:port；空串清除
curl -s -X POST http://127.0.0.1:9980/api/proxy/set \
  -H 'Content-Type: application/json' -d '{"proxy":"127.0.0.1:7890"}'

# 清空 jar / 爬虫 / 站点表（之后必须重新 loadConfig）
curl -s -X POST http://127.0.0.1:9980/api/cache/clear
```

### 5.6 手动代理（一般无需使用）

```bash
curl -s -X POST http://127.0.0.1:9980/api/sites/qq/proxy-local \
  -H 'Content-Type: application/json' \
  -d '{"params":{"do":"img","url":"https://...","ext":"..."}}'
```

返回数组，元素规则：

| Java 类型 | JSON |
|---|---|
| `String`/`Number`/`Boolean` | 原样 |
| `ByteArray` | Base64 字符串 |
| 其他 | `toString()` |
| 爬虫返回 `null` | `null` |

---

## 6. 爬虫输出的 JSON 结构（SpiderResult）

`homeContent` / `homeVideoContent` / `categoryContent` / `detailContent` / `searchContent` 的字符串内容：

```json
{
  "class": [
    {"type_id": "1", "type_name": "电影"},
    {"type_id": "2", "type_name": "电视剧"}
  ],
  "filters": {
    "1": [
      {"key": "area", "name": "地区", "init": "全部",
       "value": [{"n": "全部", "v": ""}, {"n": "大陆", "v": "大陆"}]}
    ]
  },
  "list": [
    {
      "vod_id": "m123",
      "vod_name": "繁花",
      "vod_pic": "https://img/xxx.jpg",
      "vod_remarks": "更新至30集",
      "vod_year": "2023",
      "vod_area": "大陆",
      "type_name": "国产剧",
      "vod_play_from": "qq$$$mgtv",
      "vod_play_url": "第01集$https://v.qq.com/...#第02集$https://..."
    }
  ],
  "pagecount": 12,
  "page": 1,
  "limit": 20,
  "total": 240
}
```

字段说明（Kotlin `Result.kt` 与 Rust `models.rs` 一致）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `class` | array | 分类列表，元素 `{type_id, type_name}` |
| `filters` | object | 筛选表，key 为 `type_id`，值为 `{key,name,init,value[{n,v}]}` |
| `list` | array | 影片列表，`vod_*` 为 **snake_case** |
| `header` | object | 请求头（可选） |
| `playUrl` | string | 预处理播放地址（可选） |
| `parse` / `jx` | number | 解析标志（0/1） |
| `flag` | string | 线路名（播放接口回包） |
| `url` | string\|array\|object | 播放地址，支持三种形态 |
| `pagecount` | number | 总页数 |
| `code` / `msg` | number/string | 站点自定义状态 |
| `warning` | string | **宿主注入**，非爬虫字段（见下） |

`vod_play_from` / `vod_play_url` 用 `$$$` 分线路、`#` 分集、`$` 分“集名/地址”。

### 6.1 warning 包装（重要）

当某站点 `spider.init(ext)` 抛异常但列表仍可用时，宿主返回对象而非字符串：

```json
{"ok":true,"result":{"data":"{\"list\":[...]}","warning":"RuntimeException: login failed"},"error":null}
```

- 客户端必须同时兼容 **string** 与 **`{data, warning}`** 两种形态。
- `warning` 会一直保留到该站点某次 `init` 成功才被清除。
- `playerContent` **不做**此包装。

---

## 7. 本地 HTTP `/proxy` 端点

### 7.1 请求

```
GET  http://127.0.0.1:9978/proxy?do=img&url=<urlencoded>&ext=<...>
POST http://127.0.0.1:9978/proxy        (application/x-www-form-urlencoded)
```

- 仅 `/proxy` 与 `/proxy/*` 有效，其余 404。
- 参数合并顺序：**query/表单在前，请求头覆盖同名项**（`collectParams`）。
- 支持 `OPTIONS`（CORS 预检，204）。
- 注意端口以 `hello.proxyPort` 为准；默认 9978。

### 7.2 分发与回退

```
JarLoader.proxyInvoke(params)            // 静态 com.github.catvod.spider.Proxy.proxy(Map)
   ↓ 为空/失败
Host.proxyHandle → recent 站点的 spider.proxyLocal(Map)
   ↓ 仍为空
500 "proxy error"
```

因此：**使用 `/proxy` 前先 `POST /api/sites/{siteKey}/recent`**，否则只走静态 `Proxy.proxy`。

### 7.3 爬虫返回值（`Array<Any>`）的四种形态

| 形态 | 处理 |
|---|---|
| `okhttp3.Response` | 透传状态码、Content-Type、头、流 |
| `[302, contentType, url]` | 返回重定向（301/302/307/308） |
| `[code, contentType, InputStream]` | 流式转发（无 length 用 chunked） |
| `[code, contentType, body, headersMap]` | 附带自定义头；`content-length` 用于定长响应 |

- `body` 可为 `InputStream` / `ByteArray` / `String`。
- 自动附加 `Access-Control-Allow-Origin: *` 等 CORS 头。
- 不透传 `content-length` / `transfer-encoding` / `connection`（由 NanoHTTPD 管理）。

---

## 8. 典型调用序列

```text
[进程启动]
  ← hello (api/apiPort/proxy/proxyPort)
→ POST /api/config/load {data,url,cfgId}      (60s)
→ POST /api/proxy/set {proxy}                  (可选)
→ POST /api/sites/{key}/recent
→ POST /api/sites/{key}/home {filter:true}     (60s)  → 分类 + 筛选 (+ 推荐)
→ POST /api/sites/{key}/home-video             (60s)  → 推荐列表
→ POST /api/sites/{key}/category {tid,pg,...}  (60s)
→ POST /api/sites/{key}/search {key,quick,pg}  (90s)
→ POST /api/sites/{key}/detail {ids:[vodId]}   (60s)
→ POST /api/sites/{key}/player {flag,id,...}   (45s)
→ POST /api/shutdown （或 stdin quit）
```

---

## 9. 实现细节与坑（对接前必读）

1. **HTTP only**：stdout 只输出 hello；stdin 只接受 `quit`/EOF。旧版按 `id` 收发的 stdio 客户端必须升级。
2. **并发**：NanoHTTPD 每个请求一个线程；同一站点可能被并发调用，`getSpider` 返回的是**按 `md5(jar)+siteKey` 缓存的共享实例**，爬虫自身需线程安全。
3. **双层 JSON**：业务 `result` 是字符串，需二次解析；`categoryContent` 的 `extend` 值必须能转字符串。
4. **布尔参数是真的布尔**：`"filter": 1` 会被 `toBooleanStrictOrNull()` 判 false。
5. **首次调用会下载 jar**：`https` 地址按 `Paths.jar(url)` 缓存；配置带 `;md5;` 时优先用缓存命中；下载失败会回退旧缓存，无缓存则报 `爬虫初始化失败`。
6. **jar 相对路径**必须有 `loadConfig.url` 作为基准，否则 `无法解析jar相对路径` 且该站点退化为**空 `Spider`（全部返回 `{}`）**，不报错。
7. **只有 `csp_` 开头的站点会加载真实爬虫**：`api` 不以 `csp_` 开头时直接返回基类 `Spider`，所有方法回 `"{}"`。
8. **jar 入口约定**：`com.github.catvod.spider.<api去掉csp_>` 的无参构造；可选静态 `com.github.catvod.spider.Init.init()` 与 `Proxy.proxy(Map)`。
9. **`init` 失败不致命**：异常降级为 `initWarnings`，列表/详情仍可用，通过 `warning` 透出。
10. **`/api/shutdown` / `quit` 后不要复用**：宿主 `halt(0)`，HTTP 端口与 stdin 管道关闭，客户端应重建进程。
11. **本机无鉴权**：API 只监听 127.0.0.1，但同一台机器上的任何进程都能调用；不要把端口映射到外网。
12. **UTF-8**：请求体按 UTF-8 解析（与 `Content-Type` 的 charset 无关），客户端应以 UTF-8 发送 JSON；响应同样带 `charset=utf-8`。
13. **日志位置**：stderr + `%AppData%\TV-Multiplatform\cache\log\spider-host.log`（10MB 滚动，保留 5 份）；HTTP 访问与业务耗时落 `HttpApiServer` logger，入参截断 800 字符（`loadConfig` 只记 url/长度）。

---

## 10. 客户端调用示例（Rust，与仓库实现同构）

```rust
// 1) 启动：宿主 stdout 输出的 hello 由后台线程解析，api 地址存入 api_base
let host = SpiderHost::spawn(&java, &jar, &jvm_args)?;

// 2) 业务调用：request() 内部 POST /api/rpc，并等待 api 地址就绪（计入超时）
let raw = host.request(
    "searchContent",
    serde_json::json!({"siteKey": "qq", "key": "繁花", "quick": false, "pg": "1"}),
    std::time::Duration::from_secs(90),
)?;

// 3) 内容类方法通常用 request_spider_result()，自动处理 result 为
//    "<json 字符串>" 或 {data, warning} 两种形态
let parsed = host.request_spider_result(
    "searchContent",
    serde_json::json!({"siteKey": "qq", "key": "繁花", "quick": false, "pg": "1"}),
    std::time::Duration::from_secs(90),
)?;

// 4) 退出：POST /api/shutdown，失败时退回 stdin quit
host.shutdown();
```

> 仓库实现中 `SpiderHost::request()` 已封装 HTTP 调用、超时、错误解包与 `api_base` 等待；业务层只需继续调用 `host.request(...)` / `host.request_spider_result(...)`，调用方式与旧版保持一致。
