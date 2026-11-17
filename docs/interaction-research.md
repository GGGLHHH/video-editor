# 选择与取消：交互调研（2026-11-17）

依据：主流剪辑器官方文档 / 快捷键表，加上人机交互的经典文献。没查到官方出处的单独标注。

## 主流剪辑器的做法

| 项目 | 做法 |
|---|---|
| 全选 / 取消全选 | Premiere（第三方快捷键表）、Resolve、Kdenlive：Ctrl+A / Ctrl+Shift+A；Final Cut Pro：Cmd+A / Shift+Cmd+A；Shotcut 取消用 Ctrl+D |
| Esc | Kdenlive：从任何工具退回选择工具；Premiere：拖动中按 Esc 取消拖动（社区说法，非官方）；Resolve / Shotcut / 剪映未查到「Esc 取消选择」的官方说明 |
| 点击 | Final Cut Pro、Shotcut：单击选中；Cmd/Ctrl 点击切换加选 / 减选；Shift 点击选首尾之间的连续一段；拖矩形框选。Final Cut Pro 的 Option 点击：选中但不移动播放头 |
| 框选加选 | 旧版 Premiere 手册：Shift 框选时未选的加入、已选的移除；Kdenlive：Shift+拖动框选 |
| 点空白处 | Resolve 手册：清空选择 |
| 链接的音视频 | Premiere「链接选择」开时音视频一起选，Alt 点击只选一边 |
| 选中样式 | Final Cut Pro：黄色描边，选区间时更粗（Apple 官方）；Resolve：红色描边（第三方手册镜像），有用户嫌太粗、挡住片段边界 |
| 选择跟随播放头 | Resolve 17 起默认关，在 Timeline 菜单里打开 |
| 预览上直接操作 | Final Cut Pro：角手柄等比、边手柄只改一边；Shotcut：Ctrl 旋转、Alt 临时关吸附。「Shift 等比、Alt 从中心缩放」是图形软件惯例，剪辑器文档里没找到出处 |

## 文献

- Shneiderman 1983，*Direct Manipulation*，IEEE Computer 16(8)：对象持续可见，操作快速、可逆。doi:10.1109/MC.1983.1654471
- Hutchins, Hollan, Norman 1985，*Direct Manipulation Interfaces*，HCI 1(4)：「执行鸿沟 / 评估鸿沟」。选中高亮属于后者，帮用户看清系统状态。doi:10.1207/s15327051hci0104_2
- Raskin 2000，*The Humane Interface*：尽量去掉模式；去不掉的做成按住才生效（准模式）。修饰键就属于这一类。
- Nielsen 十条可用性原则：系统状态可见；用户控制与自由（要有「紧急出口」）。
- Bier & Stone 1986，*Snap-dragging*，SIGGRAPH：吸附的经典做法。doi:10.1145/15886.15912
- 视频剪辑界面研究：
  - Casares 等 2002 *Silver*（DIS；DOI 有两种说法，引用前请到 ACM 核对）
  - Berthouzoz 等 2012（SIGGRAPH）
  - Rubin 等 2013（UIST）
  - Huber 等 2019 *B-Script*（CHI）
  - Fried 等 2019（SIGGRAPH）
- 规范：
  - Windows 拖放接口检测到 Esc 就取消、不放下（Microsoft Learn `IDropSource::QueryContinueDrag`）
  - WCAG 1.4.11：表示状态的元素与相邻颜色对比度不低于 3:1

没找到专门研究「剪辑时间轴选择反馈」的 CHI / UIST 论文。

另外做了一条调研里没明说、但不做就会出问题的：选中多个片段时，拖其中任意一个，整组在各自轨道上一起平移（多选时不换轨；往前拖到 0 为止；撞上没选中的片段就整组停住）。不然 Ctrl+A 后拖动只会动一个片段。

## 建议与进度

| 优先级 | 建议 | 依据 | 状态 |
|---|---|---|---|
| 必须 | Esc 分层：拖到一半时取消拖动（回到原位、不记撤销）；否则取消选择 | Windows 拖放、Nielsen「紧急出口」 | ✅ 已做（时间轴、预览都支持） |
| 必须 | Ctrl+A 全选、Ctrl+Shift+A 取消全选 | Premiere、Resolve、Kdenlive | ✅ 已做 |
| 必须 | 选中样式醒目：2 px 高饱和描边，加亮填充，预览选中框用同一个颜色 | Final Cut Pro、WCAG 1.4.11 | ✅ 已做：2 px 黄描边，整体蒙黄，标题栏黄底深字，预览选中框同色 |
| 必须 | 修饰键按住才生效，松开立即恢复 | Raskin | ✅ Alt 不吸附就是按住才生效 |
| 建议 | Ctrl 点击切换加选 / 减选；Shift 点击选一段 | Final Cut Pro、Shotcut | ✅ 已做。Shift 点击选的是起点片段到被点片段之间的一块：两者所在轨道之间的每条轨上，和两者合起来的时间范围有交集的片段（同一轨时就是连续一段）；起点是最后一次单击或 Ctrl 点击的片段 |
| 建议 | 空白处拖矩形框选；按住 Shift / Ctrl 框选时在原选择上加选 | Premiere、Kdenlive | ✅ 已做；框到一半按 Esc 回到框之前的选择 |
| 建议 | 链接的音视频默认一起选，Alt 点击只选一边 | Premiere | 不适用：视频的声音就在视频片段里，没有拆出来的音频片段。以后支持「分离音频」时再做 |
| 建议 | 预览上的手柄：角手柄等比、边手柄非等比，Alt 临时关闭吸附 | Final Cut Pro、Shotcut、Bier | ✅ 角手柄本来就是等比；Alt 临时不吸附已做，时间轴也一样。边手柄要等画面属性支持横竖分开缩放后再做 |
| 必须 | 预览上点选：选中图层只有角手柄优先，其余都选鼠标下最上面的图层（被上层盖住的地方点不到下层；要拖下层，就点它没被盖住的地方，或先在时间轴上选中它再拖角） | 图形软件惯例；用户实测发现「选了铺满的底层以后点不到上层小画面」 | ✅ 已修 |
| 可选 | 「选择跟随播放头」开关，默认关 | Resolve | 暂不做：Resolve 默认也关；要先有「目标轨道」的概念（播放头跟随选哪条轨），等有这个需求再加 |
| 可选 | 鼠标悬停时给弱提示，和选中样式明显区分 | — | ✅ 已做：半透明白色细描边 |
| 可选 | 拖动时在原位置留一个半透明影子 | — | ✅ 已做：半透明底色加虚线框 |
