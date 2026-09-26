# MEMORY（跨会话长期事实）

## anydoc SmartArt / Drawing 形状 → PNG 导出（路线图）

**触发**：2026-09-07 转 `D:\doc_files\图表和流程图测试.docx` 时，2 个流程图只输出"流程图测试 / 流程图测试2"文字标题，**没有图**——`diagram_blocks` 只抽文本节点，不渲染布局；`a:blip`/`imagedata` 之外的非位图形状一律不输出。

### 当前覆盖

- `src/formats/docx/content.rs` 走两条 inline 路径：
  - **SmartArt**（`dgm:relIds`）：`diagram_blocks()` → `shared::drawingml::diagram_blocks()` → 只返回 `dgm:t` 文本节点（`Block::Paragraph`），不出图。
  - **Drawing 形状**（`wps:wsp` / `wpg:wgp`）：未识别为图像来源，**完全跳过**；通用 `a:blip`/`VML imagedata` 才走 `Inline::Image`。
- 现有 `assets.rs` 在 Windows 上把 WMF/EMF 走 GDI/GDI+，其他平台调 `soffice --convert-to png`（2026-09-07 加的）。

### 方案对比（2026-09-08 更新：用户要求跨平台，**方案 D 已否决**）

| 方案 | 概述 | 跨平台 | 外部依赖 | 覆盖率 | 质量 | 工作量 |
|---|---|---|---|---|---|---|
| **E. LibreOffice HTML 导出**（已实测可行，**跨平台首推**） | `soffice --convert-to html` 把每个图形渲成独立 GIF 落同目录；HTML `<img name="形状名">` 直接映射回原形状 | ✓ Win/Linux/macOS | LO | 高（Chart + SmartArt 全出；浮动 Shape 被**拆分**为多张） | 好（GIF 256 色够用，饼图/SmartArt 无可见色带） | **最小** |
| A. 纯 Rust 解析 dgm/wps → SVG → resvg | 解析 `dgm:layoutDef` + `dgm:pt` 算坐标；形状库覆盖矩形/圆/箭头/连接线 | ✓ | 无 | 中（SmartArt 100+ 布局仅 5–10；Office 私有扩展难全） | 高（矢量） | 大（数周） |
| ~~D. Word COM `SaveAs2(wdFormatHTML)`~~ | ~~Word 渲染每个图形为独立 PNG 落 `<name>.files/`；`<img v:shapes="...">` 映射~~ | ✗ Windows+Word | Word | 高（= Word 渲染；浮动 Shape **合成一张**） | 高（PNG ≈192dpi） | 最小 |
| B/C. LO 转 PNG / EMF 中转 | 把形状段复制成临时 docx 调 `soffice --convert-to png`；或构造 EMF 走 `wmf.rs` | ✓/✗ | LO | 高（取决于 LO） | 高 | 中 |

### 方案 D 的实测证据（2026-09-07，`图表和流程图测试.docx`）

1. **`SaveAs2($html, 8)`（wdFormatHTML=8）产出 9 张 PNG** 到 `doc.files/`：
   `image001..009.png`。尺寸实测：`002/004` = 1110×649（两个 Chart）、`006/008` ≈ 1110×648（两个 SmartArt）、`009` = 799×210（3 个浮动 Shape 组合的流程图）。渲染 ≈192 dpi。
2. **HTML 里的映射线索**：`<img width=400 height=105 src="doc.files/image009.png" v:shapes="矩形_x0020_5 椭圆_x0020_6 直接箭头连接符…">`
   → `v:shapes` 列出该 PNG 包含的全部形状名，可与 Word COM 的 `Shape.Name` 对上（`_x0020_` 是空格转义）。
3. **Word COM 能拿精确坐标**（`probe-word-pdf.ps1` 实测，单位 point）：
   - `Shapes.Item(i)`：`Left` / `Top` / `Width` / `Height`；`Anchor.Information(3)` = 页码
   - `InlineShapes.Item(i)`：`Range.Information(3)` 页码，`(5)` 页内 x，`(6)` 页内 y
   - 该文档 2 页：2 Chart(p1: x=90,y=75.4 / y=356.25)、2 SmartArt(p2: y=75.75 / y=354.75)、3 浮动 Shape(p2: 矩形 L=37.6 T=25.6、椭圆 L=264.15 T=20.1、箭头 L=109.6 T=55.8)
   - 类型常量：`wdInlineShapeChart=12`、`wdInlineShapeSmartArt=15`
4. **不可用的路（别踩）**：
   - `Shape.CopyAsPicture()` 在 PowerShell COM 下报 `does not contain a method named 'CopyAsPicture'` → 剪贴板路线不通。
   - 本机**无** PDF 渲染依赖：`magick`/`mutool`/`pdftoppm`/`gswin64c` 全无；venv 无 `fitz`/`pypdfium2`/`PIL` → **PDF 中间层 + 裁剪路线要装外部依赖，不划算**（Word 已装的机器上直接走 D）。
5. **降级路径不变**：无 Word / 非 Windows → 回到现有 `diagram_blocks` 文本化 + `log::warn!`，与 `word_doc_to_docx` 的现有降级风格一致。
6. 本机 **LibreOffice 已装**：`C:\Program Files\LibreOffice\program\`（其 `python.exe` 也在，可作 UNO 备选）。

### 方案 E 实测证据（2026-09-08，`图表和流程图测试.docx`）

1. **产物**：`soffice --convert-to html --outdir <dir> <src>` →
   `<name>.html` + 7 张 `<name>_html_<hash>.gif`。
2. **HTML 映射**（`name` 即形状名，比 Word 的 `v:shapes` 更直接，一图一 `name`）：
   ```html
   <img src="..._html_e31289c.gif" name="图表 1" />                                     ← Chart 1
   <img src="..._html_3c70aaad.gif" name="图表 2" />                                    ← Chart 2
   <img src="..._html_e0055b8f.gif" name="形状1" alt="形状1" width="554" height="323" /> ← SmartArt 1
   <img src="..._html_2f3eca30.gif" name="形状2" alt="形状2" width="554" height="323" /> ← SmartArt 2
   <img src="..._html_62957afe.gif" name="形状5" ... width="96"  height="96" />          ← 浮动形状（矩形）独立
   <img src="..._html_f1057767.gif" name="形状4" ... width="96"  height="96" />          ← 浮动形状（椭圆）独立
   <img src="..._html_63cf8383.gif" name="形状3" ... width="206" height="5" />           ← 浮动形状（箭头连接符）独立
   ```
3. **`src` 是 URL-encoded 文件名**（`%E5%9B%BE...` = 图表…），需解码到磁盘文件名。
4. **质量实测**：`3c70aaad.gif`（饼图）颜色平滑、文字清晰；`e0055b8f.gif`（SmartArt 5 块）颜色一致、边缘干净。GIF 256 色够用，无可见色带。
5. **已知差异（vs 方案 D）**：
   - **浮动形状被拆成多张独立 GIF**（不像 Word 合成一张 799×210 的流程图）。下游可选：
     a) markdown 逐张显示（简单，但流程图被拆散）
     b) **应用层合成**（推荐）：读 docx 原 XML 的 `<wps:wsp>` + `<wp:positionH/V>` 拿形状坐标，用 `image` crate 把多张 GIF 按坐标拼成单张 PNG。
   - 格式是 GIF 不是 PNG（任何现代 viewer 都支持；要 PNG 可加 LO filter 选项或转完后用 `image` 转）。
6. **降级路径**：LO 不在场 → 现有 `diagram_blocks` 文本化 + `log::warn!`，与 `unix_backend::metafile_to_png` 现有降级风格一致（同一 `soffice_command()` 探测可复用）。

### 关键决策点（待用户拍板）

1. **路线**：E / A / E+A 混合？（D 已否决——不跨平台）
2. **浮动形状**：a) 拆分显示 / b) 应用层合成单张（推荐）？
3. **格式**：GIF 直接用 / 转 PNG（需加 `image` crate）/ LO filter 直接出 PNG？
4. **范围**：只 SmartArt，还是把 `wps:wsp`（流程图、文本框、标注、形状）一起覆盖？建议一起，模型层复用一套 `Inline::Image`。
5. **降级**：LO 不在场回到现有 `diagram_blocks` 文本化 + warn（推荐）。
6. **CI**：需装 LO 的 job，或 `#[ignore]` 本地跑？

### 实施里程碑（方案 E）

1. 新增 `src/shared/diagram.rs`：`capture_diagrams(bytes, format) -> Result<HashMap<ShapeId, Vec<u8>>>`
   - 复用 `wmf.rs::unix_backend::soffice_command()` 探测（跨平台）
   - 临时目录 → 写 `input.docx` → `soffice --convert-to html --outdir <work>` → 解析 `<name>.html` 的 `<img name src>` → URL-decode 读回字节
2. `docx/content.rs::inline_drawing`：在 `dgm:relIds` 与 `wps:wsp` 分支调 `capture_diagrams`；LO 不在场走原 `diagram_blocks` 降级
3. **浮动形状应用层合成**（可选）：解析 `<wps:wsp>` + `<wp:positionH/V>` 拿坐标，用 `image` crate 合成单张 PNG
4. 夹具：`tests/fixtures/diagram/` 放 `图表和流程图测试.docx` + 期望产物
5. CI：`cargo test --lib` 全过；新增 `cargo test --test docx_diagrams`（LO 装好才跑，或 `#[ignore]`）

### 风险与开放问题

- **LO 私有布局**：SmartArt 的 `dgm:layoutDef` 是 OOXML 公开的，但 Word 实际渲染常依赖私有 `dgm:layoutDef` 变体；走 LO 渲染最稳。
- **形状坐标**：方案 A 自己算坐标要解决"emf 用 dpi 决定像素大小"的问题；B/C 不涉及。
- **图片大小**：流程图通常是页面级 PNG，markdown 引用时需要 `width=` 控制；建议保留 LO 默认输出（150 dpi 左右），由用户在 markdown viewer 里调。
- **跨文档引用**：跨文档的 SmartArt 数据 XML 单独 part，按 rels 拼——已在 `rel_part()` 暴露。
- **pptx**：PPT 里的 SmartArt 走 `src/formats/pptx/mod.rs`，复用 `shared/drawingml::diagram_blocks`——**2026-09-08 用户明确决定不接**（"不需要"）。如后续要接：pptx 的 drawing 是 `p:graphicFrame` 而非 `w:drawing`，复用 `diagram.rs` 需再加一套"按 `p:spTree`/`p:graphicFrame` 分组 + 隔离渲染"的逻辑。

### 关联改动（实施时要触）

- `Cargo.toml`：可能加 `zip` 已用、`quick-xml` 已用；A 方案需 `resvg`、`usvg`。
- `src/shared/wmf.rs::unix_backend`：复用 `soffice_command()` 探测。
- `src/shared/mod.rs`：新增 `pub mod diagram;`。
- `src/formats/docx/content.rs`：新增 inline 路径。
- `src/formats/pptx/mod.rs`（二阶段）：同样加 diagram 路径。

### 触发会话

- 用户原文："需要"（确认要把 SmartArt/Shape → PNG 加进路线图）。
- 上一轮回复路线图选项 + "要不要我把这块加进路线图？"