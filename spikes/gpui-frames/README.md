# gpui-frames（一次性实验）

验证路线 A 的前提：FFmpeg 解出的帧逐帧交给 GPUI 显示，能否稳定跑满片源帧率、内存不涨。
结论写在 `docs/superpowers/specs/2026-11-17-mvp-design.md`「实验结果」一节。**不进正式代码。**

## 跑

```sh
cargo build --release
# 测试素材（仓库根目录下 media/，不入库）
ffmpeg -f lavfi -i "testsrc2=size=1920x1080:rate=60" -t 300 -c:v h264_nvenc -b:v 12M -pix_fmt yuv420p ../../media/test_1080p60.mp4

./target/release/gpui-frames ../../media/test_1080p60.mp4 --seconds 60 --log logs/look.csv
# 4K，排除管道瓶颈：
./target/release/gpui-frames ../../media/test_2160p60.mp4 --seconds 60 --hwdec --preload 60
python analyze.py logs/look.csv
```

窗口左上角实时显示：解码 / 显示帧率、覆盖、解码迟到、卡顿次数、每秒最大帧间隔、内存。到时间自动退出。
