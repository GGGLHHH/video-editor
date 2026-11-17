//! 一次性实验（throwaway）：验证路线 A 的前提——
//! FFmpeg 解出的 BGRA 帧，每帧包成 RenderImage 交给 GPUI 显示，能不能稳定跑满片源帧率、内存不涨。
//!
//! 用法：gpui-frames <视频文件> [--seconds 300] [--log logs/run.csv] [--hwdec]
//!
//! 数据流：解码线程（ffmpeg 子进程 → 按片源时间戳节拍）→ 「最新帧」槽 → UI 取走 → 下一次绘制
//! 统计：
//!   produced    解码线程按节拍交出的帧
//!   displayed   UI 实际绘制到的新帧
//!   overwritten 槽里的帧还没被 UI 取走就被下一帧覆盖
//!   late        解码线程赶不上节拍（解码本身慢）
//!   stutters    相邻两次新帧绘制间隔 > 1.5 帧时长
//!   ws / private 进程工作集 / 私有内存（MB）

use gpui::{
    App, Application, Bounds, Context, ImageId, ObjectFit, RenderImage, SharedString, Window,
    WindowBounds, WindowOptions, div, img, prelude::*, px, rgb, rgba, size,
};
use image::{Frame, RgbaImage};
use std::{
    fs::{self, File},
    io::{BufWriter, Read, Write as _},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    thread,
    time::{Duration, Instant},
};

struct Args {
    input: PathBuf,
    seconds: u64,
    log: PathBuf,
    hwdec: bool,
    preload: usize,
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let mut input = None;
    let mut seconds = 300;
    let mut log = PathBuf::from("logs/run.csv");
    let mut hwdec = false;
    let mut preload = 0;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => seconds = it.next().and_then(|s| s.parse().ok()).expect("--seconds N"),
            "--log" => log = it.next().map(PathBuf::from).expect("--log PATH"),
            "--hwdec" => hwdec = true,
            "--preload" => preload = it.next().and_then(|s| s.parse().ok()).expect("--preload N"),
            _ => input = Some(PathBuf::from(a)),
        }
    }
    Args {
        input: input.expect(
            "用法: gpui-frames <视频文件> [--seconds N] [--log PATH] [--hwdec] [--preload N]",
        ),
        seconds,
        log,
        hwdec,
        preload,
    }
}

struct VideoInfo {
    width: u32,
    height: u32,
    fps: f64,
}

fn probe(path: &PathBuf) -> VideoInfo {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .expect("运行 ffprobe 失败");
    let s = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = s.trim().split(',').collect();
    let (num, den) = parts[2].split_once('/').unwrap_or((parts[2], "1"));
    VideoInfo {
        width: parts[0].parse().unwrap(),
        height: parts[1].parse().unwrap(),
        fps: num.parse::<f64>().unwrap() / den.parse::<f64>().unwrap(),
    }
}

#[derive(Default)]
struct Counters {
    produced: AtomicU64,
    overwritten: AtomicU64,
    late: AtomicU64,
}

struct Shared {
    latest: Mutex<Option<Arc<RenderImage>>>,
    counters: Counters,
    stop: AtomicBool,
    child: Mutex<Option<Child>>,
}

/// 解码线程：ffmpeg 输出原始 BGRA，按片源帧率节拍放进「最新帧」槽。
fn spawn_decoder(args: &Args, info: &VideoInfo, shared: Arc<Shared>, wake: smol::channel::Sender<()>) {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-stream_loop", "-1"]);
    if args.hwdec {
        cmd.args(["-hwaccel", "cuda"]);
    }
    cmd.arg("-i")
        .arg(&args.input)
        .args(["-an", "-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .stdout(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = cmd.spawn().expect("启动 ffmpeg 失败");
    eprintln!("ffmpeg 子进程 PID {}", child.id());
    let mut stdout = child.stdout.take().unwrap();
    *shared.child.lock().unwrap() = Some(child);

    let (w, h) = (info.width, info.height);
    let frame_bytes = (w * h * 4) as usize;
    let frame_dur = Duration::from_secs_f64(1.0 / info.fps);
    let preload = args.preload;

    thread::Builder::new()
        .name("decoder".into())
        .spawn(move || {
            // --preload N：先读 N 帧进内存再循环送，排除 ffmpeg 管道带宽的影响，只测 GPUI
            let mut cache: Vec<Vec<u8>> = Vec::new();
            for _ in 0..preload {
                let mut buf = vec![0u8; frame_bytes];
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                cache.push(buf);
            }
            if preload > 0 {
                eprintln!("已预读 {} 帧进内存", cache.len());
            }
            let start = Instant::now();
            let mut n: u32 = 0;
            loop {
                if shared.stop.load(Relaxed) {
                    break;
                }
                let buf = if cache.is_empty() {
                    let mut buf = vec![0u8; frame_bytes];
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    buf
                } else {
                    // 每次复制一份、建新的 RenderImage（新 id），保证 GPUI 每帧都真的重新上传
                    cache[n as usize % cache.len()].clone()
                };
                // GPUI 的 RenderImage 约定内部是 BGRA，类型名虽是 RgbaImage，字节原样放进去
                let image = RgbaImage::from_raw(w, h, buf).unwrap();
                let render = Arc::new(RenderImage::new(vec![Frame::new(image)]));

                let target = start + frame_dur * n;
                let now = Instant::now();
                if now < target {
                    thread::sleep(target - now);
                } else if now - target > frame_dur {
                    shared.counters.late.fetch_add(1, Relaxed);
                }
                n += 1;

                if shared.latest.lock().unwrap().replace(render).is_some() {
                    shared.counters.overwritten.fetch_add(1, Relaxed);
                }
                shared.counters.produced.fetch_add(1, Relaxed);
                let _ = wake.try_send(());
            }
        })
        .unwrap();
}

fn memory_mb() -> (f64, f64) {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
        (
            pmc.WorkingSetSize as f64 / 1048576.0,
            pmc.PagefileUsage as f64 / 1048576.0,
        )
    }
}

struct Player {
    shared: Arc<Shared>,
    frame_dur: Duration,
    current: Option<Arc<RenderImage>>,
    prev: Option<Arc<RenderImage>>,
    last_drawn: Option<ImageId>,
    last_draw_at: Option<Instant>,
    displayed: u64,
    stutters: u64,
    max_gap: Duration,
    stats: SharedString,
}

impl Player {
    /// UI 线程拿到新帧：换上，并把「上上帧」从贴图图集里删掉（上一帧可能还在屏幕上，留一拍）。
    fn take_latest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.shared.latest.lock().unwrap().take() else {
            return;
        };
        if let Some(old) = self.prev.take() {
            let _ = window.drop_image(old);
        }
        self.prev = self.current.replace(next);
        cx.notify();
    }
}

impl Render for Player {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(cur) = &self.current {
            if self.last_drawn != Some(cur.id) {
                self.last_drawn = Some(cur.id);
                self.displayed += 1;
                let now = Instant::now();
                if let Some(t) = self.last_draw_at {
                    let gap = now - t;
                    if gap > self.frame_dur.mul_f32(1.5) {
                        self.stutters += 1;
                    }
                    self.max_gap = self.max_gap.max(gap);
                }
                self.last_draw_at = Some(now);
            }
        }

        let video = match &self.current {
            Some(i) => img(i.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .size_full()
            .relative()
            .bg(rgb(0x101010))
            .child(video)
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .p_2()
                    .rounded_md()
                    .bg(rgba(0x000000b0))
                    .text_color(rgb(0xffffff))
                    .text_sm()
                    .font_family("Consolas")
                    .child(self.stats.clone()),
            )
    }
}

fn main() {
    let args = parse_args();
    let info = probe(&args.input);
    eprintln!(
        "输入 {}：{}x{} @ {:.3} fps，跑 {} 秒，日志 {}",
        args.input.display(),
        info.width,
        info.height,
        info.fps,
        args.seconds,
        args.log.display()
    );
    if let Some(dir) = args.log.parent() {
        fs::create_dir_all(dir).unwrap();
    }

    let shared = Arc::new(Shared {
        latest: Mutex::new(None),
        counters: Counters::default(),
        stop: AtomicBool::new(false),
        child: Mutex::new(None),
    });
    let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
    spawn_decoder(&args, &info, shared.clone(), wake_tx);

    let frame_dur = Duration::from_secs_f64(1.0 / info.fps);
    let seconds = args.seconds;
    let log_path = args.log.clone();
    let fps = info.fps;

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1600.), px(900.)), cx);
        let shared_ui = shared.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    // 有新帧就换上
                    cx.spawn_in(window, async move |this, cx| {
                        while wake_rx.recv().await.is_ok() {
                            if this.update_in(cx, |p: &mut Player, w, cx| p.take_latest(w, cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();

                    // 每秒统计一次，写日志；到时间退出
                    let log_path = log_path.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let mut log = BufWriter::new(File::create(&log_path).unwrap());
                        writeln!(
                            log,
                            "t,produced_ps,displayed_ps,overwritten,late,stutters,max_gap_ms,ws_mb,private_mb"
                        )
                        .unwrap();
                        let start = Instant::now();
                        let (mut last_p, mut last_d) = (0u64, 0u64);
                        let (ws0, pv0) = memory_mb();
                        let mut first_ws = None;
                        for t in 1..=seconds {
                            cx.background_executor().timer(Duration::from_secs(1)).await;
                            let r = this.update_in(cx, |p, _w, cx| {
                                let c = &p.shared.counters;
                                let produced = c.produced.load(Relaxed);
                                let (ws, pv) = memory_mb();
                                let line = format!(
                                    "{t},{},{},{},{},{},{:.1},{:.0},{:.0}",
                                    produced - last_p,
                                    p.displayed - last_d,
                                    c.overwritten.load(Relaxed),
                                    c.late.load(Relaxed),
                                    p.stutters,
                                    p.max_gap.as_secs_f64() * 1000.0,
                                    ws,
                                    pv
                                );
                                p.stats = format!(
                                    "{t:>4}s  解码 {:>3}/s  显示 {:>3}/s  目标 {fps:.0}\n覆盖 {}  解码迟到 {}  卡顿 {}  最大间隔 {:.1}ms\n工作集 {ws:.0}MB  私有 {pv:.0}MB",
                                    produced - last_p,
                                    p.displayed - last_d,
                                    c.overwritten.load(Relaxed),
                                    c.late.load(Relaxed),
                                    p.stutters,
                                    p.max_gap.as_secs_f64() * 1000.0,
                                )
                                .into();
                                // 最大间隔按秒重置，日志里看每秒的最坏情况
                                p.max_gap = Duration::ZERO;
                                last_p = produced;
                                last_d = p.displayed;
                                cx.notify();
                                (line, ws, pv, produced, p.displayed, p.stutters)
                            });
                            let Ok((line, ws, pv, produced, displayed, stutters)) = r else {
                                return;
                            };
                            writeln!(log, "{line}").unwrap();
                            log.flush().unwrap();
                            println!("{line}");
                            // 前 10 秒是预热，内存基线从第 10 秒算
                            if t == 10 {
                                first_ws = Some((ws, pv));
                            }
                            if t == seconds {
                                let el = start.elapsed().as_secs_f64();
                                let (bws, bpv) = first_ws.unwrap_or((ws0, pv0));
                                let summary = format!(
                                    "# 汇总: {el:.0}s, 解码 {produced} 帧 ({:.2}/s), 显示 {displayed} 帧 ({:.2}/s), 卡顿 {stutters}, 工作集 {bws:.0}->{ws:.0}MB, 私有 {bpv:.0}->{pv:.0}MB",
                                    produced as f64 / el,
                                    displayed as f64 / el,
                                );
                                writeln!(log, "{summary}").unwrap();
                                log.flush().unwrap();
                                println!("{summary}");
                            }
                        }
                        let _ = this.update_in(cx, |p: &mut Player, _w, cx| {
                            p.shared.stop.store(true, Relaxed);
                            if let Some(mut child) = p.shared.child.lock().unwrap().take() {
                                let _ = child.kill();
                            }
                            cx.quit();
                        });
                    })
                    .detach();

                    Player {
                        shared: shared_ui,
                        frame_dur,
                        current: None,
                        prev: None,
                        last_drawn: None,
                        last_draw_at: None,
                        displayed: 0,
                        stutters: 0,
                        max_gap: Duration::ZERO,
                        stats: "启动中…".into(),
                    }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
