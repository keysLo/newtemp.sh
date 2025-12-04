# newtemp.sh

一个使用 Rust 编写的轻量级临时文件分享服务，类似 temp.sh：
上传文件后生成一次性访问地址，链接最多可被访问 3 次，并在达到访问次数或超出保留时间后自动删除文件与链接。

## 运行

运行前可以将环境变量写入配置文件（默认 `config.env`，通过 `CONFIG_FILE` 指定其他路径），启动时会自动加载：

```bash
cat > config.env <<'ENV'
ADDRESS=0.0.0.0:8080          # 监听地址（默认 0.0.0.0:8080）
STORAGE_DIR=./data            # 文件存储目录（默认 ./data）
DEFAULT_TTL_SECS=3600         # 链接与文件默认保留时长（秒）
CLEANUP_INTERVAL_SECS=60      # 清理过期文件的周期（秒）
MAX_DOWNLOADS=3               # 每个链接最大访问次数（默认 3）
MAX_UPLOAD_GB=1               # 最大上传文件大小（GB，默认 1GB）
ENV
```bash
# 可选：配置环境变量
export ADDRESS=0.0.0.0:8080          # 监听地址（默认 0.0.0.0:8080）
export STORAGE_DIR=./data            # 文件存储目录（默认 ./data）
export DEFAULT_TTL_SECS=3600         # 链接与文件默认保留时长（秒）
export CLEANUP_INTERVAL_SECS=60      # 清理过期文件的周期（秒）
export MAX_DOWNLOADS=3               # 每个链接最大访问次数（默认 3）

cargo run
```

## 上传示例

使用 `curl` 的 multipart 上传：

```bash
curl -F "file=@/path/to/file" http://localhost:8080/upload
```

响应示例：

```json
{
  "url": "/d/2d017dd9-7f7f-4f94-8a9a-21d3fdd7c2f3",
  "expires_in_seconds": 3600,
  "remaining_downloads": 3
}
```

使用返回的 `url` 下载文件（最多 3 次，超过次数或过期后文件与链接都会删除）：

```bash
curl -O http://localhost:8080/d/2d017dd9-7f7f-4f94-8a9a-21d3fdd7c2f3
```

服务会自动在后台周期性清理过期的文件与记录。
