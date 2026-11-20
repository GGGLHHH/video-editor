# 特效插件

插件就是一个 `.wgsl` 文件（WebGPU 着色语言），放进插件目录就能当画面特效用。

- 插件目录：`%APPDATA%\video-editor\effects`。第一次启动时会建好，并放两个示例：`反色.wgsl`、`老电视.wgsl`。
- 文件名（去掉 `.wgsl`）就是特效名，最多 48 字节。
- 放进新文件、或者改了文件以后，在素材面板「特效」最下面点「重新载入」。
- 用法和内置特效一样：点一下插件卡片，就在播放头处加一段特效片段。它作用于下面所有轨道的画面，强度在属性面板「特效」里调。

## 写法

一个插件只需要写一个函数：

```wgsl
fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> {
    // c      这个像素已经合成好的颜色（RGB，0–1）
    // p      像素坐标（左上角是 0,0）
    // dims   画面大小（像素）
    // amount 强度（0–1，属性面板里的滑条）
    // t      特效片段开始后过了几秒
    return mix(c, vec3(1.0) - c, amount);   // 反色
}
```

- 返回值会被夹到 0–1。
- 要看周围的像素，用 `bd(vec2<i32>(x, y))` 读已经合成好的画面；越界的坐标会被夹到画面边上。
- 可以用内置着色器里的辅助函数，例如 `hash21(p)` 生成噪点。
- 自己写的函数名别和内置的重名。内置名字有 `bd`、`spiral_avg`、`hash21`、`blend_channel`、`fs`、`vs` 等。

## 例子

老电视（扫描线 + 轻微闪烁）：

```wgsl
fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> {
    let line = select(1.0, 0.6, (i32(p.y) % 3) == 0);
    let flicker = 1.0 - 0.05 * sin(t * 60.0);
    return mix(c, c * line * flicker, amount);
}
```

左右镜像（右半边照出左半边）：

```wgsl
fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> {
    if p.x < dims.x / 2.0 {
        return c;
    }
    let mirrored = bd(vec2<i32>(i32(dims.x - 1.0 - p.x), i32(p.y)));
    return mix(c, mirrored, amount);
}
```

## 出错时

- 载入时就会编译检查。写错的插件不会出现在列表里，并弹出提示，说明是哪个文件、错在哪一行。
- 工程里用到的插件被删了或者编译不过时，这道特效直接跳过，画面不受影响，也不会让程序崩掉。
- 预览和导出用的是同一套编译结果，两边效果一样。

## 和 OFX 的区别

达芬奇、Premiere 的插件是 OFX 等 C 接口的动态库，可以带自己的界面和多个参数。这里的插件只有一个强度参数，没有自己的界面，只能处理画面。好处是写起来简单、跨平台，而且不会让程序崩溃。
