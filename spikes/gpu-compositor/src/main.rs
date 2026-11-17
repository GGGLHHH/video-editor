//! 一次性实验：显卡合成多层画面的性能。
//!
//! ```text
//! gpu-compositor bench  [--out 1920x1080] [--frames 600]      不开窗口：上传 + 合成 + 读回 每帧耗时
//! gpu-compositor window [--out 1920x1080] [--seconds 20]      GPUI 窗口：60fps 合成 → 读回 → 显示，测实际显示帧率
//! gpu-compositor export [--out 1920x1080] [--frames 600] [--encoder h264_nvenc] [--dir 目录]
//!                                                              合成 → 管道 → ffmpeg 编码，测导出速度
//! ```

mod compositor;

use compositor::{Compositor, FrameTiming, make_layer, placements};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, RenderImage, Styled as _,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, px, size,
};
use std::io::Write as _;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Opts {
    mode: String,
    out: (u32, u32),
    frames: u64,
    seconds: f64,
    encoder: String,
    dir: std::path::PathBuf,
}

fn parse() -> Opts {
    let mut a = std::env::args().skip(1);
    let mut o = Opts {
        mode: a.next().unwrap_or_else(|| "bench".into()),
        out: (1920, 1080),
        frames: 600,
        seconds: 20.0,
        encoder: "h264_nvenc".into(),
        dir: std::env::temp_dir(),
    };
    while let Some(k) = a.next() {
        let v = a.next().unwrap_or_default();
        match k.as_str() {
            "--out" => {
                let (w, h) = v.split_once('x').expect("--out WxH");
                o.out = (w.parse().unwrap(), h.parse().unwrap());
            }
            "--frames" => o.frames = v.parse().unwrap(),
            "--seconds" => o.seconds = v.parse().unwrap(),
            "--encoder" => o.encoder = v,
            "--dir" => o.dir = v.into(),
            _ => panic!("未知参数 {k}"),
        }
    }
    o
}

/// 4 层：背景铺满、两个画中画（源画面和输出同尺寸，在显卡上缩小）、台标 256×256。
fn scene(out: (u32, u32)) -> (Vec<(u32, u32)>, Vec<Vec<u8>>) {
    let sizes = vec![out, out, out, (256, 256)];
    let pixels = sizes.iter().enumerate().map(|(i, &(w, h))| make_layer(i as u32, w, h)).collect();
    (sizes, pixels)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn report(label: &str, mut v: Vec<f64>) {
    let avg = v.iter().sum::<f64>() / v.len() as f64;
    let (p50, p99, max) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 1.0));
    println!("{label:<16} 平均 {avg:6.2} ms   p50 {p50:6.2}   p99 {p99:6.2}   最长 {max:6.2}");
}

fn report_timings(t: &[FrameTiming]) {
    report("上传", t.iter().map(|x| x.upload_ms).collect());
    report("合成+读回", t.iter().map(|x| x.render_readback_ms).collect());
    report("合计", t.iter().map(|x| x.upload_ms + x.render_readback_ms).collect());
}

fn bench(o: &Opts) {
    let (sizes, pixels) = scene(o.out);
    let mut c = Compositor::new(&sizes, o.out);
    println!("显卡：{}   输出 {}x{}   4 层（3 层 {}x{} 源 + 256 台标）", c.adapter_info, o.out.0, o.out.1, o.out.0, o.out.1);
    let refs: Vec<&[u8]> = pixels.iter().map(Vec::as_slice).collect();
    let mut buf = Vec::new();
    for n in 0..30 {
        c.compose(&refs, &placements(n, o.out), &mut buf); // 预热
    }
    let t0 = Instant::now();
    let timings: Vec<FrameTiming> = (0..o.frames).map(|n| c.compose(&refs, &placements(n, o.out), &mut buf)).collect();
    let wall = t0.elapsed().as_secs_f64();
    report_timings(&timings);
    println!("吞吐 {:.1} 帧/秒（{} 帧 {:.2} 秒）", o.frames as f64 / wall, o.frames, wall);
    let path = o.dir.join(format!("compose_{}x{}.png", o.out.0, o.out.1));
    save_png(&buf, o.out, &path);
    println!("合成结果截图：{}", path.display());
}

fn save_png(bgra: &[u8], (w, h): (u32, u32), path: &std::path::Path) {
    let mut rgba = bgra.to_vec();
    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let _ = std::fs::write(path, png_bytes(&rgba, w, h));
}

fn png_bytes(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
    )
    .expect("编码 png");
    out
}

fn export(o: &Opts) {
    let (sizes, pixels) = scene(o.out);
    let mut c = Compositor::new(&sizes, o.out);
    let refs: Vec<&[u8]> = pixels.iter().map(Vec::as_slice).collect();
    let file = o.dir.join(format!("export_{}x{}_{}.mp4", o.out.0, o.out.1, o.encoder));
    let mut ff = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra", "-s"])
        .arg(format!("{}x{}", o.out.0, o.out.1))
        .args(["-r", "60", "-i", "-", "-c:v", &o.encoder, "-b:v", "20M", "-pix_fmt", "yuv420p"])
        .arg(&file)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("启动 ffmpeg");
    let mut stdin = ff.stdin.take().unwrap();
    println!("显卡：{}   导出 {}x{} {} 帧，编码器 {}", c.adapter_info, o.out.0, o.out.1, o.frames, o.encoder);
    let mut buf = Vec::new();
    let mut timings = Vec::new();
    let mut write_ms = Vec::new();
    let t0 = Instant::now();
    for n in 0..o.frames {
        timings.push(c.compose(&refs, &placements(n, o.out), &mut buf));
        let tw = Instant::now();
        stdin.write_all(&buf).expect("写管道");
        write_ms.push(tw.elapsed().as_secs_f64() * 1000.0);
    }
    drop(stdin);
    let status = ff.wait().expect("等 ffmpeg");
    let wall = t0.elapsed().as_secs_f64();
    report_timings(&timings);
    report("写管道(等编码)", write_ms);
    println!(
        "导出 {:.1} 帧/秒，{:.2} 倍实时（{} 帧 {:.2} 秒，ffmpeg {}）→ {}",
        o.frames as f64 / wall,
        o.frames as f64 / 60.0 / wall,
        o.frames,
        wall,
        status,
        file.display()
    );
}

// ---------- GPUI 窗口 ----------

struct Viewer {
    rx: Receiver<(u64, Arc<RenderImage>)>,
    pending: std::collections::VecDeque<(u64, Arc<RenderImage>)>,
    skipped: u64,
    image: Option<Arc<RenderImage>>,
    retired: Option<Arc<RenderImage>>,
    start: Instant,
    seconds: f64,
    last_new: Option<Instant>,
    intervals: Vec<f64>,
    renders: u64,
    timings: Arc<Mutex<Vec<FrameTiming>>>,
    done: bool,
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.renders += 1;
        // 和正式引擎一样：合成线程提前排帧（带帧号），界面按时钟挑当前该显示的那一帧
        while self.pending.len() < 4 {
            match self.rx.try_recv() {
                Ok(f) => self.pending.push_back(f),
                Err(_) => break,
            }
        }
        let due = (self.start.elapsed().as_secs_f64() * 60.0) as u64;
        let mut newest = None;
        while self.pending.front().is_some_and(|(n, _)| *n <= due) {
            if newest.is_some() {
                self.skipped += 1;
            }
            newest = self.pending.pop_front().map(|(_, f)| f);
        }
        if let Some(next) = newest {
            let now = Instant::now();
            if let Some(prev) = self.last_new.replace(now) {
                self.intervals.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            }
            if let Some(old) = self.retired.take() {
                cx.drop_image(old, Some(window));
            }
            self.retired = std::mem::replace(&mut self.image, Some(next));
        }
        let elapsed = self.start.elapsed().as_secs_f64();
        if elapsed >= self.seconds && !self.done {
            self.done = true;
            let shown = self.intervals.len() + 1;
            println!("窗口 {elapsed:.1} 秒：显示新画面 {shown} 次 = {:.2} fps；界面重绘 {} 次；排好但来不及显示被跳过 {} 帧", shown as f64 / elapsed, self.renders, self.skipped);
            let mut iv = self.intervals.clone();
            let over = iv.iter().filter(|&&x| x > 25.0).count();
            report("显示间隔", iv.clone());
            println!("间隔 >25ms（掉帧）{over} 次，p99.9 {:.2} ms", pct(&mut iv, 0.999));
            report_timings(&self.timings.lock().unwrap());
            cx.quit();
        }
        window.request_animation_frame();
        div().size_full().bg(gpui_kit::black()).children(self.image.clone().map(|i| img(i).size_full()))
    }
}

fn producer(out: (u32, u32), tx: SyncSender<(u64, Arc<RenderImage>)>, timings: Arc<Mutex<Vec<FrameTiming>>>) {
    let (sizes, pixels) = scene(out);
    let mut c = Compositor::new(&sizes, out);
    eprintln!("显卡：{}   窗口预览 {}x{}", c.adapter_info, out.0, out.1);
    let refs: Vec<&[u8]> = pixels.iter().map(Vec::as_slice).collect();
    let mut buf = Vec::new();
    for n in 0u64.. {
        let t = c.compose(&refs, &placements(n, out), &mut buf);
        timings.lock().unwrap().push(t);
        let Some(rgba) = image::RgbaImage::from_raw(out.0, out.1, std::mem::take(&mut buf)) else { return };
        let frame = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
        // 队列满就阻塞：最多领先界面几帧
        if tx.send((n, frame)).is_err() {
            return;
        }
    }
}

fn window(o: &Opts) {
    let (out, seconds) = (o.out, o.seconds);
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        let (tx, rx) = sync_channel(4);
        let timings = Arc::new(Mutex::new(Vec::new()));
        let t2 = timings.clone();
        std::thread::spawn(move || producer(out, tx, t2));
        let opts = WindowOptions {
            titlebar: Some(TitlebarOptions { title: Some("gpu-compositor".into()), ..Default::default() }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(720.)), cx))),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |_, cx| {
            cx.new(|_| Viewer {
                rx,
                pending: std::collections::VecDeque::new(),
                skipped: 0,
                image: None,
                retired: None,
                start: Instant::now(),
                seconds,
                last_new: None,
                intervals: Vec::new(),
                renders: 0,
                timings,
                done: false,
            })
        })
        .expect("开窗口");
    });
}

fn main() {
    let o = parse();
    match o.mode.as_str() {
        "bench" => bench(&o),
        "export" => export(&o),
        "window" => window(&o),
        m => panic!("未知模式 {m}"),
    }
}
