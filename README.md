<div align="center">

# DEMStudio

### Shape the terrain. Render the landscape.

**从高程数据到精致地形影像。**

一款专注于数字高程模型（DEM）的 Windows 桌面渲染工具。  
用直观的工作区塑造地形、调整光影，并将成果输出为 PNG 或 TIFF；原图正射可附带地理信息。

![DEMStudio 工作区](https://raw.githubusercontent.com/SpDDoggy/DEMStudio/92e6cff89cfe862cc178805eafde74578e0d5e04/assets/readme/hero.png)

`Windows` · `Rust` · `Tauri 2` · `OpenGL` · `GLSL` · `v0.2.0`

</div>

---

## 为地形而设计

DEMStudio 将高度场渲染与地图制作连接起来：既可以独立制作具有光影和材质表现的地形图像，也可以输出正射 GeoTIFF，继续在 ArcGIS Pro 等 GIS 软件中叠加道路、水系、注记和其他空间图层。

**不是通用 GIS，也不试图取代它。** DEMStudio 专注于一件事：让地形渲染更可控、更精致，并让成果更容易进入后续制图流程。

## 核心能力

| | |
| :--- | :--- |
| **地形重建** | 方块、多边形、双线性与双三次曲面；高度缩放与偏移。 |
| **外观塑造** | 统一材质、高程色带、连续 / 离散配色与图像贴图。 |
| **光照与成像** | 太阳、天空、软阴影、渐进采样、曝光与整体调色。 |
| **交互工作区** | 以地形为中心的视口、浮动参数面板和视角导航。 |
| **多种数据输入** | PNG、JPEG、TIFF / BigTIFF 与 ASC 高程栅格。 |
| **专业输出** | PNG / TIFF、8-bit / 16-bit；当前视图或原图正射，自定义尺寸与可选地理信息。 |

## 从渲染到地图

**DEM → DEMStudio → GeoTIFF → GIS → 地图作品**

1. **导入** DEM 或高度图，建立地形。
2. **塑造** 曲面、赋色、光照与相机视角。
3. **渲染** 当前视图，或按完整原图范围输出正射成果。
4. **制图** 将 GeoTIFF 加入 ArcGIS Pro，叠加湖泊、道路、地名等图层并完成排版。

> **实际案例 · 云南滇池**  
> DEMStudio 输出带地理参考的 GeoTIFF，随后在 ArcGIS Pro 中完成湖泊、道路、地名与版式设计。




## 获取与运行

下载 [DEMStudio 0.2.0](https://github.com/SpDDoggy/DEMStudio/releases/tag/v0.2.0)：提供 Windows x64 安装包和便携版。也可从下列完整源码构建。

**环境要求**

- Rust 与 MSVC 工具链
- Visual Studio C++ Build Tools
- Microsoft Edge WebView2 Runtime
- 支持 OpenGL 3.3 Compatibility Profile 的显卡及驱动

```powershell
git clone https://github.com/SpDDoggy/DEMStudio.git
cd DEMStudio
cargo run --release --locked --manifest-path src-tauri/Cargo.toml
```

仅构建可执行文件：

```powershell
cargo build --release --locked --manifest-path src-tauri/Cargo.toml
```

构建结果位于 `build/tauri/release/demstudio.exe`。首次构建需要下载 Cargo 依赖。界面使用原生 HTML、CSS 和 JavaScript，无需 npm 构建步骤。

## 基本操作

| 操作 | 方式 |
| :--- | :--- |
| 旋转视角 | 鼠标右键拖动 |
| 平移视角 | `Shift` + 鼠标右键拖动 |
| 缩放 | 鼠标滚轮 |
| 切换标准视角 | 方向立方体 / 底部视角工具 |
| 输出图像 | 导出面板中选择当前视图或正射输出 |

### 地理信息导出

选择“原图正射”后，源数据自带坐标系时可勾选“导出地理信息”。PNG 和 TIFF 均可使用；PNG 的坐标信息保存在同名 `.png.aux.xml` 中，导入 GIS 时请保留配套文件。自定义坐标系的 TIFF 也可能附带 `.tif.aux.xml`。

原图正射保持完整源范围，按输出宽高计算像元大小；阴影外扩同步平移地理原点。当前视图保留相机取景。大栅格采用有界预览与源精度分页导出，预览细节与最终文件精度可分别选择。

## 技术实现

DEMStudio 采用轻量的桌面 UI 与原生渲染核心分离的架构：

```text
DEMStudio/
├── src/ui/                         # HTML / CSS / JS 界面与交互
├── src-tauri/src/main.rs           # Tauri 桌面入口与 IPC
├── src-tauri/src/renderer/         # 栅格处理、GPU 渲染与输出
│   └── shaders/                   # GLSL 高度场求交、光照与采样
└── assets/readme/                 # README 展示素材
```

- **Rust + Tauri 2**：桌面应用与原生模块。
- **OpenGL + GLSL**：高度场求交、光照与渐进渲染。
- **原生栅格处理**：读取高度数据、生成预览并输出地理参考成果。

## 当前状态

**v0.2.0**

支持地形导入、交互渲染、源精度分页导出及带地理参考的 PNG / TIFF 输出。仓库包含 Rust / OpenGL 渲染核心、GLSL、栅格读取和文件输出的完整构建依赖。

---

<div align="center">

**DEMStudio**  
*Terrain rendering for visual exploration and cartographic workflows.*

</div>
