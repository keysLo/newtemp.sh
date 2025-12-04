# newtemp.sh

一个使用 Rust 编写的轻量级临时文件分享服务，类似 temp.sh：
上传文件后生成一次性访问地址，链接最多可被访问 3 次，并在达到访问次数或超出保留时间后自动删除文件与链接。

## 运行

运行前可以将环境变量写入配置文件（默认 `config.env`，通过 `CONFIG_FILE` 指定其他路径），启动时会自动加载：

```bash
cat > config.env <<'ENV'
ADDRESS=0.0.0.0:8080          # 监听地址（默认 0.0.0.0:8080）
STORAGE_DIR=./data            # 文件存储目录（默认 ./data）
DEFAULT_TTL_MINS=60           # 链接与文件默认保留时长（分钟）
CLEANUP_INTERVAL_MINS=1       # 清理过期文件的周期（分钟）
MAX_DOWNLOADS=3               # 每个链接最大访问次数（默认 3）
URL_PREFIX=                   # （可选）自定义完整链接前缀，例如 https://google.com:123
UPLOAD_PAGE_ENABLED=true      # （默认 true）是否启用内置上传页面
UPLOAD_PASSWORD=changeme      # 上传密码（上传页面与 /upload 接口均需携带）
MAX_UPLOAD_GB=1               # 最大上传文件大小（GB，默认 1GB）
ENV
```bash
# 可选：配置环境变量
export ADDRESS=0.0.0.0:8080          # 监听地址（默认 0.0.0.0:8080）
export STORAGE_DIR=./data            # 文件存储目录（默认 ./data）
export DEFAULT_TTL_MINS=60           # 链接与文件默认保留时长（分钟）
export CLEANUP_INTERVAL_MINS=1       # 清理过期文件的周期（分钟）
export MAX_DOWNLOADS=3               # 每个链接最大访问次数（默认 3）
export URL_PREFIX=                   # （可选）自定义完整链接前缀，例如 https://google.com:123
export UPLOAD_PAGE_ENABLED=true      # （默认 true）是否启用内置上传页面
export UPLOAD_PASSWORD=changeme      # 上传密码（上传页面与 /upload 接口均需携带）

cargo run
```

启动后可直接在浏览器打开根路径（如 `http://localhost:8080/`）使用内置上传页面，输入配置的上传密码即可完成上传。

## 上传示例

使用 `curl` 的 multipart 上传：

```bash
curl -F "password=changeme" -F "file=@/path/to/file" http://localhost:8080/upload
```

响应示例：

```json
{
  "url": "https://google.com:123/d/2d017dd9-7f7f-4f94-8a9a-21d3fdd7c2f3",
  "expires_in_minutes": 60,
  "remaining_downloads": 3
}
```

使用返回的 `url` 下载文件（最多 3 次，超过次数或过期后文件与链接都会删除）：

```bash
curl -O http://localhost:8080/d/2d017dd9-7f7f-4f94-8a9a-21d3fdd7c2f3
```

服务会自动在后台周期性清理过期的文件与记录。
