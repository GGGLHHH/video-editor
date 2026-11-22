# 程序图标

- `icon.svg`：图标本身（256 × 256）。青色渐变圆角方形（主题强调色 `#00C8D2`）上放白色图形。
  图形来自 [Material Symbols](https://github.com/google/material-design-icons) 的 `movie_edit`（rounded、填充，
  npm 包 `@material-symbols/svg-400@0.49.0`），按 Apache-2.0 使用，许可全文见 `LICENSE-material-symbols.txt`。
- `icon.ico`：16、20、24、32、40、48、64、128、256 九个尺寸（PNG 编码的条目），由 `icon.svg` 用 resvg 画出来。
- `icon-256.png`：256 像素的 PNG（文档、发布页用）。
- `app.rc`：Windows 资源，`build.rs` 编进 exe，编号 1。

换图标：改 `icon.svg`，再重新生成 `icon.ico` 和 `icon-256.png`（决策 139 里记了做法）。
