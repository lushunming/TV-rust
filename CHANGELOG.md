# 未发布（rust 分支）
- 修复 UC 站点 `NoClassDefFoundError: Could not initialize class com.github.catvod.api.UCApi$Loader`：第三方爬虫在刷新 token 失败时会把错误 JSON 原样写入 `~/TV/.uctoken`，下次 `UCApi` 初始化解析 `refresh_token` 触发 NPE，导致类初始化永久失败（同 JVM 内 UC/百度/夸克搜索站全部报错）。宿主新增启动自愈：首次加载爬虫前校验 token 缓存，错误响应或无法解析的内容重命名为 `.uctoken.bad-<时间戳>` 隔离，`~/TV/.uc` 登录 cookie 保留；新增 5 个单测覆盖有效/错误/空 token/损坏 JSON/文件缺失
- **spider-host API 层改为 HTTP**：业务调用统一走本地 `POST http://127.0.0.1:<apiPort>/api/rpc` 与语义化 `/api/...` 端点（配置、首页、分类、搜索、详情、播放、代理设置），不再使用 stdio JSON 行协议；stdout 仅保留一行 hello 握手，stdin 仅处理 `quit`/EOF
- 修复 HTTP 请求体中文乱码：NanoHTTPD 对不带 `charset` 的 `application/json` 默认按 US-ASCII 解码，导致搜索词/站点名/extend 值变成“������”；现在 API 层直接按 UTF-8 读取原始 body（与 Content-Type 是否带 charset 无关），并补充中文站点名、中文 `cfgId`、中文搜索词测试
- **新增 Swagger 调试文档**：`/swagger`（Swagger UI）与 `/openapi.json`（OpenAPI 3.0.3），swagger-ui 资源随 fat jar 打包、离线可用；`/api/health`、`/api/server/info`、`/api/sites` 可直接在浏览器测试
- API 默认监听 `127.0.0.1:9980`（9980–9999 被占用时自动顺延，仍失败则退回系统临时端口），实际端口写进 hello 的 `apiPort`/`api`；代理端口 `9978` 保持不变
- `serverInfo` 增加 `version`/`apiPort`/`apiBase`/`swagger`/`openapi` 字段
- **tv-rust 客户端改为 HTTP**：`SpiderHost` 使用 reqwest blocking（`no_proxy`、连接超时、按请求超时），自动等待 hello 下发 api 地址；`shutdown` 优先调用 `/api/shutdown`，失败再退回 stdin `quit`
- 新增 `HttpApiServerTest` 冒烟测试：健康检查、OpenAPI 路径、Swagger 页面/静态资产、未知方法 400、配置加载与站点列表、站点不存在 400
- `spider-host/docs/spider-host-api.md` 全面改写为 HTTP 版（含 Swagger、curl 示例、状态码与迁移说明）
- 首页视频列表改为滚动加载：删掉分页按钮/"加载更多"，滚到底自动请求下一页，底部显示"加载中/继续向下滚动/已全部加载"；站点未上报 pagecount 时靠"下一页为空"判定到底
- 字体整体调大（正文 16 / 按钮 15 / 标题 22），海报单元 150px、名称 15px
- 站源选择改为下拉框：带搜索过滤（名称/key/类型）、当前项高亮、右侧显示站点数与 `csp_` 类型；不再用横向长条堆按钮
- 跨平台打包：新增 `packaging/package-linux.sh`（tar.gz）、`packaging/package-macos.sh`（TVRust.app + dmg），Windows 脚本改为读取共用清单 `packaging/common/jlink-modules.txt`
- 新增 `.github/workflows/package.yml`：Windows/macOS/Ubuntu 22.04 三平台矩阵，各平台自行 `cargo build` + `jlink`
- 新增 `.gitattributes`：`.sh` 强制 LF（避免 CRLF 导致 bash 报错），`.ps1` 为 CRLF
- 修复打包脚本中文注释被 PS 5.1 当 GBK 解析而“吃掉下一行代码”的问题（曾导致模块清单丢失 `jdk.crypto.ec`），Windows 脚本现在保持纯 ASCII
- 修复“启动播放器失败 … os error 2”：mpv 分支为支持额外参数按空格切路径，`D:\Program Files\mpv\mpv.exe` 被截成 `D:\Program`；改为先判整串是否为文件，再支持引号 / 在 `.exe` 处切分，并补单测
- 播放器路径不存在时自动回退到探测到的播放器，仍找不到才报错并给出提示；错误信息与 playerLog 记录实际使用的 exe
- 修复打包版“首页无内容”：jlink 运行时缺 `jdk.crypto.ec`/`jdk.charsets`，导致所有 HTTPS 爬虫 init 失败、homeContent 返回空；补入 `jdk.crypto.ec,jdk.charsets,jdk.zipfs,jdk.naming.dns,jdk.security.auth,jdk.unsupported.desktop,jdk.net`
- 修复首页丢弃推荐内容：`homeContent` 同时返回分类与推荐列表（Wogg 255 条、SeedHub 20 条），之前只取分类；现直接用推荐列表首屏展示，且 `homeVideoContent` 为空时不再清空
- 新增 RPC 级日志：`rpc >> method params`（loadConfig 只记 url/长度）与 `rpc << ok 耗时 result`（截断 2000 字符），便于判断“站点返回空”还是“解析失败”
- 新增本地日志，便于定位问题：
  - `%AppData%\TV-Multiplatform\cache\rust\tv-rust.log`：启动参数、配置加载、站点切换、列表条数、播放与所有 toast 提示
  - `%AppData%\TV-Multiplatform\cache\rust\spider-host.log`：宿主 stderr 原样落盘（之前打包版直接丢失），超过 4MB 自动轮转
  - `%AppData%\TV-Multiplatform\cache\log\spider-host.log`：宿主 logback 自身文件日志，带 RPC 方法与耗时
  - `playerLog.txt` 现在会收到播放器 stdout/stderr（之前直接丢弃）
- 设置页新增“诊断日志”区域：显示日志目录，可一键打开目录/playerLog
- 修复分类页整页失败：站点把数字写成字符串（如 `"pagecount": "0"`）时解析报错（Qupans 等）；`pagecount/code/parse/jx` 改为宽松数字解析，并加单测
- 网盘站 `init` 失败降噪：只留一条 WARN（`已忽略，站点继续可用`），堆栈进 DEBUG，不再误判为致命故障
- 修复爬虫 `init` 异常导致整站不可用：`init` 失败不再致命（与原版 CatVod 一致），保留站点并回传 `warning`，前端每个站点只提示一次
- 修复“视频无法展示”：宿主缓存目录依赖 `AppData` 环境变量，变量缺失时退回相对路径，导致 jar 缓存永不命中、重新下载被 TLS 拒绝，所有 `csp_` 站点空列表；现兜底到 `%USERPROFILE%\AppData\Roaming` 并打印实际目录
- 修复 jar 地址解析：先拆分 `;md5;` 再解析相对路径，避免 md5 串被当成 URL 路径
- jar 下载增加原子写入、zip/md5 校验，失败时回退已有缓存（不再整站无数据）
- tv-rust 按配置预取爬虫 jar（使用 Rust TLS 栈），写入宿主同款缓存路径
- 爬虫初始化异常不再被吞成空列表，前端以 toast 显示具体原因
- 记住上次使用的站源，重启后不再回到配置里的第一个站点
- 一体化打包：新增 `packaging/package-windows.ps1`，产出 `TVRust.exe + spider-host-all.jar + jlink runtime` 便携目录与 zip（实测 56.3 MB），用户无需安装 Java
- tv-rust 新增随包运行时与 jar 的优先探测（`runtime/bin/java.exe` → `JAVA_HOME` → PATH；jar 优先 exe 同级目录）
- spider-host 新增内置本地 HTTP 代理服务（`/proxy`，默认端口 9978，被占用自动顺延）：流式转发、Range、重定向与响应头透传，修复 jar 爬虫图片/m3u8 地址全部 404 的问题
- 宿主协议新增 `proxyPort`/`proxy` 握手字段与 `serverInfo`、`setProxy` 方法；`/proxy` 转发优先走 jar 静态 proxy，失败回退站点 `proxyLocal`
- tv-rust 解析宿主实际代理端口，播放与海报地址中的 `127.0.0.1:9978` 自动重写
- tv-rust 海报按站点 `header`（Referer/UA）加载，缓存键区分不同站点
- tv-rust 首页筛选应用配置默认值（`Filter.init`），首屏选中态与实际请求参数一致
# v1.2.5
- 升级依赖
- 修复点击搜索框异常错误
- 去除圆角
# v1.2.3
- 修复缓存jar加载
- 修复url路径解析
- 优化设置点播地址加载效果
- 优化vlc组建加载提示
- 修改控制栏动画
- 进入退出详情页时异步加载/清理资源
- 内置播放器新增缓冲进度提示
- 加载配置时显示加载指示
- 优化详情页标题展示
- 首页条件多行过滤器
- 修复日志打印资源争抢的问题
- 修复历史记录线路保存
- 优化播放器提示，新增错误提示
# v1.2.2
- 修复爬虫对象加载
- 修复vlc组件加载问题
- 设置页面播放器设置常显示输入框
- 优化播放器控制栏显示， 暂停状态下调整进度的效果
# v1.2.0
- 新的内部播放器基于vlcj
- 细节优化/bug修复
- 修复图片加载卡顿的问题
- 设置页面优化
- 新增预发布渠道
# v1.1.22
- 标题tooltip/加载指示优
- fix bug #6
- 限制窗口最小尺寸
- 使用浮动按钮返回顶部 取消首页滚动条
- 搜索建议框不能自动消失的问题
- 在搜索页进入详情页在退出的时候的搜索词问题
- 结构优化 页面清空
# v1.1.21
1. 增加日志
2. 优化首页加载更多的逻辑
3. 修复可能的初始化问题
4. 在搜索页面搜索时异常
5. 退出页面的时候清空job
6. 详情页快搜结果更新
7. 图片加载资源关闭
8. 代码清理
9. 项目结构调整
10. snackbar 调整
# v1.1.10
- 更换图片请求客户端 为 okhttp
- feat:修改默认设置，标注输出流重定向到日志文件
- fix: 修复在win10系统不显示窗口的问题
- feat: 搜索站源限制 每次搜索四个站源 更多需要点击加载更多
- 优化：site 使用categories支持过滤分类
  - 首页 分类支持滚动 增加滚动条 
  - 海外看接口支持修复
  - json解析 使用宽松模式
# 20240419
- 优化：初步优化搜索调用次数（详情页搜索）
- 优化：当不同站源的提示
- 优化：页面与逻辑的分离
- 删除无用类
- 修复：并发修改异常