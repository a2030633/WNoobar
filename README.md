# <img width="512" height="512" alt="icon_concept_b_aurora_pill" src="https://github.com/user-attachments/assets/6498ca9c-5363-4add-8dd6-3fb9efd7f760" />
WNoobar (Windows AI Agent Dynamic Island)

> **Silent by default, Delightful when active.**  
> 专为 Windows 平台打造的 **AI Agent 原生灵动岛** 悬浮感知组件。

---

## 📖 项目简介

**WNoobar** 是一款纯血原生、极轻量、高性能的 Windows 桌面灵动岛交互程序。它能在屏幕顶端以优雅的胶囊与流光动画，实时感知并可视化各大 AI Agent（如 **Google Antigravity**、**OpenAI / GitHub Codex**、**Claude Code** 等）的任务生命周期。

程序基于纯 Rust + Win32 原生分层窗口（Layered Window）与 GDI+ 渲染，不依赖任何重量级 GUI 框架或 Web 容器，编译产物为单一便携 `.exe` 文件（体积小于 1MB），待机 CPU 占用 0.0%，后台静默常驻无任何 CMD 命令行黑框干扰。

---

## ✨ 核心特性

### 1. 三段式节律交互 (Three-Beat Rhythm)
- **节拍 1 · 启动致意 (Start Pop)**：当用户发起新任务时，顶层灵动岛轻盈滑出展开，致意 1.8 秒后平滑微缩贴边。
- **节拍 2 · 执行静默期 (Silent In-Progress)**：后台思考与执行过程中，灵动岛保持极细微光贴边（仅 2.5px），呼吸光晕指示 Agent 正在工作，绝不弹窗挡住 IDE 敲代码。
- **节拍 3 · 完成交付 (Completion Pop)**：当任务彻底交付收官时，灵动岛顺滑延展出大卡片，展示任务成果摘要与耗时，展示 3.5 秒后自动收缩贴边归于待命。

### 2. 活跃会话抽屉 (Session Deck)
- 点击灵动岛左侧微标或托盘菜单，即可纵向展开「活跃会话抽屉」；
- 聚合展示近期所有 Agent 会话卡片，实时显示会话名称、步数、执行阶段与项目所属；
- 适配各 Agent 官方精美微标（Antigravity 四角星辉、OpenAI Codex 绿徽、Claude Code 等）。

### 3. 系统状态栏托盘常驻 (System Tray)
- **无黑框后台运行**：已启用 `#![windows_subsystem = "windows"]`，完全摆脱 CMD 终端窗口；
- **鼠标左键单击 / 双击**：快速展开灵动岛或收缩为贴边隐藏；
- **鼠标右键快捷菜单**：
  - 📌 **展开/收起灵动岛**
  - 📋 **活跃会话抽屉 (Session Deck)**
  - 🔄 **还原待命就绪状态**
  - ❌ **安全退出 WNoobar**
- **动态悬浮气泡 (Tooltip)**：随任务状态实时更新（待命就绪 / 处理中 / 完成 / 需确认）。

---

## 🏗️ 系统架构与目录结构

```text
f:\项目\WNoobar
├── assets/                     # 嵌入式矢量与高精位图资产
│   └── icons/                  # 官方 Agent 微标 (Antigravity, Codex, Claude, DeepSeek)
├── src/
│   ├── main.rs                 # 程序入口与生命周期守护
│   ├── core/                   # 核心业务层
│   │   ├── bus.rs              # 轻量无锁 crossbeam 事件总线 (AgentEvent)
│   │   ├── fsm/                # 有限状态机 (IslandState, EdgeDockMode)
│   │   └── watcher/            # 多 Agent 监听驱动引擎
│   │       ├── antigravity.rs  # Google Antigravity 本地 Brain 日志监听器
│   │       └── codex.rs        # OpenAI Codex 会话日志管道监听器
│   ├── ui/                     # 原生视觉与动效层
│   │   ├── capsule.rs          # 灵动岛视窗、卡片与抽屉 GDI+ 绘制器
│   │   ├── glyphs.rs           # 各 Agent 图标位图与光晕特效管理器
│   │   └── spring.rs           # RK4 阻尼弹簧物理动效引擎 (Spring Dynamics)
│   └── win32/                  # Win32 底层系统契约
│       ├── gdiplus.rs          # GDI+ C-ABI 封装与 ClearType 抗锯齿
│       └── window.rs           # 穿透分层窗口、系统托盘 (Shell_NotifyIconW)、HitTest
├── tests/
│   └── integration_test.rs     # 系统级端到端集成测试套件
├── Cargo.toml                  # Rust 依赖与发布配置
└── wnoobar.exe                 # 最终生成的单文件便携可执行程序
```

---

## 🚀 编译与运行指南

### 前置依赖
- **Rust 工具链**：1.80+ (推荐使用 `x86_64-pc-windows-msvc` 或 `x86_64-pc-windows-gnu`)
- **构建环境**：Windows 10 (1903+) 或 Windows 11

### 本地编译
```bash
# 运行自动化测试套件
cargo test

# 生产级极速构建 (开启 LTO 与符号裁剪，生成纯单文件)
cargo build --release
```

编译出的二进制文件位于：
`target/release/wnoobar.exe`（约 800KB）。

### 启动运行
直接双击运行 `wnoobar.exe` 即可：
- 无需安装，即开即用；
- 启动后自动吸附在主显示器顶部中央，并常驻于右下角任务栏托盘。

---

## 📄 文档索引

- **系统技术规格书**：请查阅本会话历史归档的 [`wnoobar_architecture.md`](file:///C:/Users/WYF/.gemini/antigravity/brain/780605e5-6f13-4d42-89d8-5c16274ac781/wnoobar_architecture.md) 获取详细的系统设计与架构图谱。
