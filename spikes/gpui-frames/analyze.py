"""汇总 gpui-frames 的测量日志：python analyze.py logs/run_xxx.csv [logs/gpu_mem_xxx.csv]"""
import csv
import statistics
import sys

rows = [r for r in csv.DictReader(l for l in open(sys.argv[1], encoding="utf-8") if not l.startswith("#"))]
warm = [r for r in rows if int(r["t"]) > 10]  # 前 10 秒预热不算
disp = [int(r["displayed_ps"]) for r in warm]
gaps = [float(r["max_gap_ms"]) for r in warm]
ws = [float(r["ws_mb"]) for r in warm]
pv = [float(r["private_mb"]) for r in warm]
print(f"秒数 {len(warm)}（去掉前 10 秒预热）")
print(f"显示帧率  平均 {statistics.mean(disp):.2f}  最低 {min(disp)}  低于 58 的秒数 {sum(d < 58 for d in disp)}")
print(f"每秒最大间隔  中位 {statistics.median(gaps):.1f}ms  最大 {max(gaps):.1f}ms  >25ms 的秒数 {sum(g > 25 for g in gaps)}")
print(f"卡顿累计 {warm[-1]['stutters']}  解码迟到累计 {warm[-1]['late']}  槽覆盖累计 {warm[-1]['overwritten']}")
print(f"工作集 {ws[0]:.0f} -> {ws[-1]:.0f} MB（最大 {max(ws):.0f}）  私有 {pv[0]:.0f} -> {pv[-1]:.0f} MB（最大 {max(pv):.0f}）")
bad = [(r["t"], r["max_gap_ms"]) for r in warm if float(r["max_gap_ms"]) > 25]
if bad:
    print("卡顿发生在（秒, 间隔ms）:", bad)

if len(sys.argv) > 2:
    vals = []
    for row in csv.reader(open(sys.argv[2], encoding="utf-8", errors="ignore")):
        try:
            vals.append(float(row[1]) / 1048576)
        except (ValueError, IndexError):
            pass
    if vals:
        print(f"进程显存（专用）{vals[0]:.0f} -> {vals[-1]:.0f} MB（最大 {max(vals):.0f}，{len(vals)} 个采样）")
