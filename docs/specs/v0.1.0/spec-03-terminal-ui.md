# AURA Terminal UI — Spec 03

**Status**: Design-ready  
**Builds on**: spec-02-interactive-runtime.md  
**Goal**: Cyber-retro terminal interface that delights developers

---

## 1) Vision

Create a terminal UI that feels like you're operating a cyberpunk AI system from the future. The interface should:

* **Feel powerful** — Like piloting a starship's AI core
* **Be approachable** — New devs and "vibe coders" can jump right in
* **Provide rich feedback** — Every action has visual confirmation
* **Look distinctive** — Instantly recognizable cyber-retro aesthetic

### Design Principles

1. **Clarity over cleverness** — Information hierarchy is king
2. **Progressive disclosure** — Simple by default, powerful when needed
3. **Responsive feedback** — Every keystroke acknowledged
4. **Forgiving UX** — Easy to undo, hard to break things

### Updated Crate Layout

Introduces `aura-terminal` as a new crate:

```
aura/
├─ aura-core          # IDs, schemas, hashing (unchanged)
├─ aura-store         # RocksDB storage (unchanged)
├─ aura-kernel        # Deterministic kernel + turn processor (unchanged)
├─ aura-node          # Router, scheduler, workers (unchanged)
├─ aura-reasoner      # Provider-agnostic + Anthropic impl (unchanged)
├─ aura-executor      # Executor trait + orchestration (unchanged)
├─ aura-tools         # ToolExecutor (fs + cmd) + sandbox (unchanged)
├─ aura-terminal      # NEW: Cyber-retro terminal UI library
├─ aura-cli           # Interactive CLI (uses aura-terminal)
└─ aura-gateway-ts    # DEPRECATED
```

---

## 2) Visual Design System

### 2.1 Color Palette

```
┌──────────────────────────────────────────────────────────────────┐
│  AURA TERMINAL COLOR SYSTEM                                      │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  PRIMARY COLORS                                                  │
│  ─────────────                                                   │
│  ██████  Cyan (#00FFFF)      - Primary accent, AI responses      │
│  ██████  Magenta (#FF00FF)   - Secondary accent, tool calls      │
│  ██████  Neon Green (#39FF14) - Success states, confirmations    │
│                                                                  │
│  SEMANTIC COLORS                                                 │
│  ───────────────                                                 │
│  ██████  Electric Blue (#00D4FF) - Links, navigation             │
│  ██████  Amber (#FFBF00)         - Warnings, requires attention  │
│  ██████  Hot Pink (#FF1493)      - Errors, denials               │
│  ██████  Purple (#9D00FF)        - System messages               │
│                                                                  │
│  BASE COLORS                                                     │
│  ───────────                                                     │
│  ██████  Deep Black (#0D0D0D)    - Background                    │
│  ██████  Dark Gray (#1A1A1A)     - Panels, cards                 │
│  ██████  Medium Gray (#333333)   - Borders, dividers             │
│  ██████  Light Gray (#888888)    - Muted text, timestamps        │
│  ██████  White (#FFFFFF)         - Primary text                  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### 2.2 Typography Hierarchy

Use Unicode box-drawing and block elements for structure. Text styling via ANSI:

| Element | Style | Color | Example |
|---------|-------|-------|---------|
| Headers | Bold + Underline | Cyan | `═══ AURA KERNEL v2.1 ═══` |
| AI Output | Regular | Cyan | Model responses |
| User Input | Bold | White | User prompts |
| Tool Names | Bold | Magenta | `[fs.read]` |
| Tool Output | Dim | Light Gray | File contents |
| Success | Bold | Neon Green | `✓ File written` |
| Warning | Bold | Amber | `⚠ Requires approval` |
| Error | Bold | Hot Pink | `✗ Permission denied` |
| System | Italic | Purple | `» Connecting...` |
| Timestamp | Dim | Light Gray | `[14:32:07]` |

### 2.3 Box Drawing Characters

```
Standard borders:     ┌─────────────────┐
                      │  Content Area   │
                      └─────────────────┘

Double borders:       ╔═════════════════╗
(important panels)    ║  Important!     ║
                      ╚═════════════════╝

Rounded borders:      ╭─────────────────╮
(friendly, casual)    │  User prompt    │
                      ╰─────────────────╯

Heavy borders:        ┏━━━━━━━━━━━━━━━━━┓
(active/focused)      ┃  ACTIVE TASK    ┃
                      ┗━━━━━━━━━━━━━━━━━┛
```

### 2.4 ASCII Art Header

```
    ╔═══════════════════════════════════════════════════════════════════╗
    ║                                                                   ║
    ║     █████╗ ██╗   ██╗██████╗  █████╗      ██████╗ ███████╗        ║
    ║    ██╔══██╗██║   ██║██╔══██╗██╔══██╗    ██╔═══██╗██╔════╝        ║
    ║    ███████║██║   ██║██████╔╝███████║    ██║   ██║███████╗        ║
    ║    ██╔══██║██║   ██║██╔══██╗██╔══██║    ██║   ██║╚════██║        ║
    ║    ██║  ██║╚██████╔╝██║  ██║██║  ██║    ╚██████╔╝███████║        ║
    ║    ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝     ╚═════╝ ╚══════╝        ║
    ║                                                                   ║
    ║    ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀      ║
    ║           Autonomous Universal Reasoning Architecture            ║
    ║                     [ KERNEL ACTIVE ]                            ║
    ║                                                                   ║
    ╚═══════════════════════════════════════════════════════════════════╝
```

### 2.5 Compact Header (Default)

```
┌─────────────────────────────────────────────────────────────────────────┐
│  ◈ AURA OS  │  Agent: dev-01  │  Session: #a7f3  │  ● Connected       │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3) Screen Layout

### 3.1 Main Interface

```
╔═════════════════════════════════════════════════════════════════════════╗
║  ◈ AURA OS  │  Agent: dev-01  │  Session: #a7f3  │  ● Connected        ║
╠═════════════════════════════════════════════════════════════════════════╣
║                                                                         ║
║  [14:32:01] YOU                                                         ║
║  ╭─────────────────────────────────────────────────────────────────╮    ║
║  │ Can you read the main.rs file and explain what it does?         │    ║
║  ╰─────────────────────────────────────────────────────────────────╯    ║
║                                                                         ║
║  [14:32:02] AURA ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━   ║
║                                                                         ║
║  I'll read the main.rs file for you.                                   ║
║                                                                         ║
║  ┌─ TOOL: fs.read ─────────────────────────────────────────────────┐   ║
║  │ path: "src/main.rs"                                             │   ║
║  └─────────────────────────────────────────────────────────────────┘   ║
║                                                                         ║
║  ┌─ OUTPUT ────────────────────────────────────────────────────────┐   ║
║  │ 1│ use aura_node::Node;                                       │   ║
║  │ 2│ use aura_store::RocksStore;                                  │   ║
║  │ 3│                                                              │   ║
║  │ 4│ #[tokio::main]                                               │   ║
║  │ 5│ async fn main() -> anyhow::Result<()> {                      │   ║
║  │  │ ... (12 more lines)                                          │   ║
║  └─────────────────────────────────────────────────────────────────┘   ║
║                                                                         ║
║  This is the entry point for the Aura Swarm server. It:                ║
║    • Initializes the RocksDB store                                     ║
║    • Creates the HTTP router                                           ║
║    • Starts the worker scheduler                                       ║
║                                                                         ║
╠═════════════════════════════════════════════════════════════════════════╣
║  ▸ Type your message... (Tab: autocomplete │ /help │ Ctrl+C: cancel)   ║
╚═════════════════════════════════════════════════════════════════════════╝
```

### 3.2 Status Bar Modes

**Normal Mode:**
```
┌─ STATUS ────────────────────────────────────────────────────────────────┐
│  ● Ready  │  Tokens: 12.4k/100k  │  Tools: 3 used  │  ⏱ 2.3s last     │
└─────────────────────────────────────────────────────────────────────────┘
```

**Processing Mode:**
```
┌─ STATUS ────────────────────────────────────────────────────────────────┐
│  ◐ Thinking...  │  ████████░░░░  │  Step 2/8  │  elapsed: 4.2s        │
└─────────────────────────────────────────────────────────────────────────┘
```

**Approval Required:**
```
┏━ APPROVAL REQUIRED ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃  ⚠ fs.write wants to modify: src/main.rs                              ┃
┃                                                                        ┃
┃  [Y] Approve   [N] Deny   [D] Show Diff   [?] More Info               ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

---

## 4) Interactive Components

### 4.1 Spinners & Progress

**Thinking Spinner Frames** (cycle at 80ms):
```
◐ ◓ ◑ ◒    (circle quarters)
⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏    (braille dots)
▁ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ ▆ ▅ ▄ ▃ ▂    (wave)
┤ ┘ ┴ └ ├ ┌ ┬ ┐    (box rotation)
```

**Progress Bar Styles:**
```
Loading:     [████████████░░░░░░░░] 60%
Streaming:   ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━▸ 
Blocks:      ▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░ 60%
Pulse:       ░░░░████░░░░    (animated)
```

**Tool Execution Indicator:**
```
┌─ fs.read ──────────────────────────────┐
│  ◐ Reading file...                     │
└────────────────────────────────────────┘
        ↓ (completes)
┌─ fs.read ──────────────────────────────┐
│  ✓ Complete (234 bytes, 12ms)          │
└────────────────────────────────────────┘
```

### 4.2 Notification Toasts

**Success:**
```
╭────────────────────────────────────────╮
│  ✓ File saved successfully             │
│    src/main.rs (1.2 KB)               │
╰────────────────────────────────────────╯
```

**Warning:**
```
╭────────────────────────────────────────╮
│  ⚠ Large file detected                 │
│    Reading first 1MB only              │
╰────────────────────────────────────────╯
```

**Error:**
```
╭────────────────────────────────────────╮
│  ✗ Command failed                      │
│    Exit code: 1                        │
│    Use /retry to try again            │
╰────────────────────────────────────────╯
```

### 4.3 Diff Display

```
┌─ DIFF: src/main.rs ─────────────────────────────────────────────────────┐
│                                                                         │
│   5   │     let config = Config::default();                            │
│   6 - │     let store = RocksStore::new(&config.data_dir)?;            │
│   6 + │     let store = RocksStore::new(&config.data_dir)              │
│   7 + │         .with_cache_size(1024 * 1024 * 64)?;                   │
│   8   │     let swarm = Swarm::new(store);                              │
│                                                                         │
│  ─────────────────────────────────────────────────────────────────────  │
│   +2 additions   -1 deletion                                           │
│                                                                         │
│  [Y] Apply   [N] Reject   [E] Edit   [?] Explain changes              │
└─────────────────────────────────────────────────────────────────────────┘
```

### 4.4 File Tree View

```
┌─ PROJECT: aura_os ──────────────────────────────────────────────────────┐
│                                                                         │
│   📁 aura_os/                                                           │
│   ├── 📁 aura-core/                                                     │
│   │   └── 📁 src/                                                       │
│   │       ├── 📄 lib.rs                                                 │
│   │       ├── 📄 types.rs ✎                                             │
│   │       └── 📄 error.rs                                               │
│   ├── 📁 aura-kernel/                                                   │
│   │   └── 📁 src/                                                       │
│   │       ├── 📄 kernel.rs ★                                            │
│   │       └── 📄 policy.rs                                              │
│   └── 📄 Cargo.toml                                                     │
│                                                                         │
│   ★ = currently viewing   ✎ = modified                                 │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 5) Animation & Feedback

### 5.1 Startup Sequence

```rust
// Animated boot sequence (500ms total)
async fn boot_sequence(term: &mut Terminal) -> Result<()> {
    // Phase 1: Logo fade-in (200ms)
    for opacity in [0.2, 0.4, 0.6, 0.8, 1.0] {
        term.render_logo(opacity)?;
        sleep(Duration::from_millis(40)).await;
    }
    
    // Phase 2: System checks (200ms)
    let checks = [
        ("Kernel", "initialized"),
        ("Store", "connected"),
        ("Reasoner", "online"),
        ("Tools", "loaded"),
    ];
    
    for (component, status) in checks {
        term.print_check(component, status)?;
        sleep(Duration::from_millis(50)).await;
    }
    
    // Phase 3: Ready prompt (100ms)
    term.print_ready_banner()?;
    
    Ok(())
}
```

**Visual Output:**
```
    ◈ AURA OS v2.1
    ══════════════════════════════════════════════════
    
    ✓ Kernel ............ initialized
    ✓ Store ............. connected  
    ✓ Reasoner .......... online
    ✓ Tools ............. loaded (7 available)
    
    ══════════════════════════════════════════════════
    Ready. Type /help for commands or start chatting.
    
```

### 5.2 Typing Indicators

**AI Thinking:**
```
AURA is thinking ◐    (spinner cycles)
```

**AI Streaming Response:**
```
AURA ◇━━━━━━━━━━━━━━━━━━━━━▸
The file contains several important...█    (cursor blinks)
```

**Tool Executing:**
```
┌─ fs.read ──────────────────┐
│  ◐ Reading src/main.rs...  │
└────────────────────────────┘
```

### 5.3 State Transitions

| From State | To State | Animation |
|------------|----------|-----------|
| Idle | Processing | Spinner appears, status bar pulses cyan |
| Processing | Tool Use | Tool card slides in from left |
| Tool Use | Tool Complete | Checkmark animation, card highlights green |
| Tool Complete | Processing | Brief pause, continue streaming |
| Processing | Complete | Final message, status returns to Ready |
| Any | Error | Red flash, error toast slides down |
| Any | Approval | Amber border highlight, modal appears |

### 5.4 Sound Cues (Optional)

| Event | Sound | Description |
|-------|-------|-------------|
| Message sent | `beep_low` | Soft confirmation |
| Response start | `beep_high` | AI begins |
| Tool success | `chime_success` | Pleasant ding |
| Tool error | `buzz_error` | Brief buzz |
| Approval needed | `alert_attention` | Two-tone alert |
| Task complete | `fanfare_mini` | Celebration |

*Note: Sounds are off by default, enabled via `/sound on`*

---

## 6) Command System

### 6.1 Slash Commands

```
┌─ AURA COMMANDS ─────────────────────────────────────────────────────────┐
│                                                                         │
│  GENERAL                                                                │
│  ────────                                                               │
│  /help [cmd]          Show help (optional: specific command)            │
│  /clear               Clear conversation history                        │
│  /status              Show system status and stats                      │
│  /quit, /exit         Exit AURA                                         │
│                                                                         │
│  NAVIGATION                                                             │
│  ──────────                                                             │
│  /history [n]         Show last n messages (default: 10)                │
│  /context             Show current context window                       │
│  /files               Show project file tree                            │
│  /search <pattern>    Search conversation history                       │
│                                                                         │
│  TOOLS                                                                  │
│  ─────                                                                  │
│  /tools               List available tools                              │
│  /approve             Approve pending action                            │
│  /deny                Deny pending action                               │
│  /diff                Show pending changes                              │
│  /undo                Undo last file change                             │
│                                                                         │
│  SESSION                                                                │
│  ───────                                                                │
│  /save [name]         Save session state                                │
│  /load <name>         Load saved session                                │
│  /export              Export conversation to markdown                   │
│  /compact             Summarize and compact context                     │
│                                                                         │
│  SETTINGS                                                               │
│  ────────                                                               │
│  /theme <name>        Switch color theme                                │
│  /sound [on|off]      Toggle sound effects                              │
│  /verbose [on|off]    Toggle verbose output                             │
│  /model <name>        Switch AI model                                   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### 6.2 Keyboard Shortcuts

```
┌─ KEYBOARD SHORTCUTS ────────────────────────────────────────────────────┐
│                                                                         │
│  INPUT                                                                  │
│  ─────                                                                  │
│  Enter             Send message                                         │
│  Shift+Enter       New line (multiline input)                          │
│  Tab               Autocomplete command/path                           │
│  Ctrl+C            Cancel current operation                             │
│  Ctrl+L            Clear screen                                         │
│  Ctrl+D            Exit (when input empty)                              │
│                                                                         │
│  NAVIGATION                                                             │
│  ──────────                                                             │
│  ↑/↓               Browse input history                                 │
│  Ctrl+R            Search input history                                 │
│  Page Up/Down      Scroll conversation                                  │
│  Home/End          Jump to start/end                                    │
│                                                                         │
│  QUICK ACTIONS                                                          │
│  ─────────────                                                          │
│  Y                 Approve (when prompted)                              │
│  N                 Deny (when prompted)                                 │
│  D                 Show diff (when prompted)                            │
│  Esc               Dismiss modal/notification                           │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### 6.3 Smart Autocomplete

```
/th█                     
   ┌────────────────┐
   │ /theme         │
   │ /thread        │  
   └────────────────┘

/theme █
        ┌────────────────────────────────────┐
        │ cyber      (current)              │
        │ minimal    Black & white          │
        │ matrix     Green on black         │
        │ synthwave  Pink & purple          │
        │ light      Light mode             │
        └────────────────────────────────────┘

src/ma█
       ┌────────────────────────────────────┐
       │ src/main.rs                        │
       │ src/manager.rs                     │
       │ src/macro_utils.rs                 │
       └────────────────────────────────────┘
```

---

## 7) Themes

### 7.1 Built-in Themes

**Cyber (Default)**
```
Background: #0D0D0D (deep black)
Primary:    #00FFFF (cyan)
Secondary:  #FF00FF (magenta)
Accent:     #39FF14 (neon green)
```

**Matrix**
```
Background: #000000 (pure black)
Primary:    #00FF00 (matrix green)
Secondary:  #008000 (dark green)
Accent:     #00FF00 (bright green)
```

**Synthwave**
```
Background: #1A1A2E (dark purple)
Primary:    #FF2E97 (hot pink)
Secondary:  #00D4FF (electric blue)
Accent:     #FFE66D (sunset yellow)
```

**Minimal**
```
Background: #1A1A1A (dark gray)
Primary:    #FFFFFF (white)
Secondary:  #888888 (gray)
Accent:     #00FF00 (green)
```

**Light** (for the brave)
```
Background: #FAFAFA (off-white)
Primary:    #1A1A1A (near black)
Secondary:  #0066CC (blue)
Accent:     #008800 (green)
```

### 7.2 Theme Configuration

```toml
# ~/.aura/themes/custom.toml

[colors]
background = "#0D0D0D"
foreground = "#FFFFFF"
primary = "#00FFFF"
secondary = "#FF00FF"
success = "#39FF14"
warning = "#FFBF00"
error = "#FF1493"
muted = "#888888"

[ui]
border_style = "rounded"  # single, double, rounded, heavy, ascii
show_icons = true
animate_spinners = true
show_timestamps = true

[behavior]
sound_enabled = false
auto_approve_reads = true
compact_tool_output = false
```

---

## 8) Responsive Layout

### 8.1 Terminal Width Breakpoints

| Width | Layout | Changes |
|-------|--------|---------|
| < 60 | Compact | No borders, minimal chrome |
| 60-80 | Normal | Standard layout |
| 80-120 | Comfortable | Full borders, icons |
| > 120 | Wide | Side panels available |

### 8.2 Compact Mode (< 60 chars)

```
◈ AURA │ ● Ready │ 12.4k tokens
────────────────────────────────────
YOU: Read the main.rs file
────────────────────────────────────
AURA: I'll read that file.

[fs.read] src/main.rs ✓

The file contains the entry point...
────────────────────────────────────
▸ _
```

### 8.3 Wide Mode (> 120 chars)

```
╔═══════════════════════════════════════════════════════╦══════════════════════════════════╗
║  ◈ AURA OS  │  Agent: dev-01  │  ● Connected          ║  📁 FILES                        ║
╠═══════════════════════════════════════════════════════╣                                  ║
║                                                       ║  aura_os/                        ║
║  [14:32:01] YOU                                       ║  ├── aura-core/                  ║
║  ╭───────────────────────────────────────────────╮    ║  │   └── src/                    ║
║  │ Can you read main.rs and explain it?          │    ║  │       ├── lib.rs             ║
║  ╰───────────────────────────────────────────────╯    ║  │       └── types.rs ✎         ║
║                                                       ║  ├── aura-kernel/               ║
║  [14:32:02] AURA ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━     ║  │   └── src/                    ║
║                                                       ║  │       └── kernel.rs ★        ║
║  I'll read the main.rs file for you.                 ║  └── Cargo.toml                 ║
║                                                       ║                                  ║
║  ┌─ TOOL: fs.read ─────────────────────────────┐     ╠══════════════════════════════════╣
║  │ path: "src/main.rs"                         │     ║  📊 STATS                        ║
║  │ ✓ 234 bytes (12ms)                          │     ║                                  ║
║  └─────────────────────────────────────────────┘     ║  Tokens: 12.4k / 100k           ║
║                                                       ║  Tools:  3 calls                 ║
║  This is the entry point...                          ║  Time:   4.2s                    ║
║                                                       ║                                  ║
╠═══════════════════════════════════════════════════════╩══════════════════════════════════╣
║  ▸ Type your message...                                                                  ║
╚══════════════════════════════════════════════════════════════════════════════════════════╝
```

---

## 9) Special Modes

### 9.1 Focus Mode

Minimal distractions for deep work:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│                                                                         │
│                                                                         │
│                     AURA is working on your request                     │
│                                                                         │
│                           ████████████████                              │
│                                                                         │
│                        Reading 3 files...                               │
│                                                                         │
│                                                                         │
│                                                                         │
│                         Press [Esc] to expand                           │
└─────────────────────────────────────────────────────────────────────────┘
```

### 9.2 Verbose/Debug Mode

For developers who want to see everything:

```
╔═════════════════════════════════════════════════════════════════════════╗
║  ◈ AURA OS  │  VERBOSE MODE  │  Agent: dev-01  │  ● Connected          ║
╠═════════════════════════════════════════════════════════════════════════╣
║                                                                         ║
║  [14:32:02.123] ← REQ ModelProvider::complete                          ║
║    model: claude-opus-4-6                                      ║
║    messages: 4                                                          ║
║    tools: 7                                                             ║
║    max_tokens: 4096                                                     ║
║                                                                         ║
║  [14:32:04.456] → RES ModelResponse                                    ║
║    stop_reason: tool_use                                               ║
║    usage: { input: 1234, output: 567 }                                 ║
║    latency: 2333ms                                                      ║
║    request_id: msg_01XYZ...                                            ║
║                                                                         ║
║  [14:32:04.458] ▶ TOOL EXEC fs.read                                    ║
║    path: src/main.rs                                                    ║
║    sandbox: /data/workspaces/dev-01/                                   ║
║    resolved: /data/workspaces/dev-01/src/main.rs                       ║
║                                                                         ║
║  [14:32:04.462] ✓ TOOL OK                                              ║
║    bytes: 234                                                           ║
║    time: 4ms                                                            ║
║                                                                         ║
╠═════════════════════════════════════════════════════════════════════════╣
║  ▸ Type your message... (/verbose off to disable)                      ║
╚═════════════════════════════════════════════════════════════════════════╝
```

### 9.3 Presentation Mode

For demos and streaming:

```
╔═════════════════════════════════════════════════════════════════════════╗
║                                                                         ║
║                           ◈ AURA OS                                     ║
║                                                                         ║
╠═════════════════════════════════════════════════════════════════════════╣
║                                                                         ║
║   USER                                                                  ║
║   ────                                                                  ║
║   Can you help me refactor this function to be more efficient?         ║
║                                                                         ║
║                                                                         ║
║   AURA                                                                  ║
║   ────                                                                  ║
║   I'll analyze the function and suggest improvements.                  ║
║                                                                         ║
║   Reading: src/utils.rs                                                ║
║   ████████████████████████████████████ ✓                               ║
║                                                                         ║
║   Here's my analysis:                                                  ║
║   • Current complexity: O(n²)                                          ║
║   • Suggested complexity: O(n log n)                                   ║
║   • Key change: Use a HashMap for lookups                              ║
║                                                                         ║
╚═════════════════════════════════════════════════════════════════════════╝
```

---

## 10) Implementation

### 10.1 Crate Architecture

The terminal UI is implemented as a **standalone library crate** (`aura-terminal`) that can be consumed by the CLI application (`aura-cli`) or any other frontend. This separation provides:

* **Reusability** — Other apps can embed the AURA terminal UI
* **Testability** — UI components can be tested in isolation
* **Clean boundaries** — UI logic separate from application logic

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           aura-cli (binary)                             │
│  • Entry point (main.rs)                                                │
│  • Arg parsing (clap)                                                   │
│  • Session management                                                   │
│  • Connects kernel ↔ terminal                                           │
├─────────────────────────────────────────────────────────────────────────┤
│                          aura-terminal (library)                        │
│  • Terminal abstraction                                                 │
│  • UI components (messages, tools, diffs)                              │
│  • Themes & styling                                                     │
│  • Input handling & autocomplete                                        │
│  • Animations & feedback                                                │
└─────────────────────────────────────────────────────────────────────────┘
```

### 10.2 Crate Structure

```
aura-terminal/                    # NEW: Terminal UI library
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Public API exports
│   ├── terminal.rs               # Terminal abstraction
│   ├── app.rs                    # Application state machine
│   ├── renderer.rs               # Main render loop
│   ├── components/
│   │   ├── mod.rs
│   │   ├── header.rs             # Header bar component
│   │   ├── message.rs            # Chat message bubbles
│   │   ├── tool_card.rs          # Tool execution cards
│   │   ├── diff.rs               # Diff viewer
│   │   ├── progress.rs           # Spinners & progress bars
│   │   ├── input.rs              # Input field
│   │   ├── status.rs             # Status bar
│   │   ├── toast.rs              # Notification toasts
│   │   ├── modal.rs              # Modal dialogs (approval, etc.)
│   │   └── file_tree.rs          # File browser panel
│   ├── themes/
│   │   ├── mod.rs                # Theme trait + loading
│   │   ├── theme.rs              # Theme struct definition
│   │   ├── cyber.rs              # Default cyber theme
│   │   ├── matrix.rs             # Matrix green theme
│   │   ├── synthwave.rs          # Synthwave theme
│   │   └── minimal.rs            # Minimal theme
│   ├── input/
│   │   ├── mod.rs
│   │   ├── handler.rs            # Key event handling
│   │   ├── history.rs            # Input history
│   │   └── autocomplete.rs       # Smart autocomplete
│   ├── layout/
│   │   ├── mod.rs
│   │   ├── responsive.rs         # Responsive breakpoints
│   │   └── panels.rs             # Panel arrangements
│   ├── animation/
│   │   ├── mod.rs
│   │   ├── spinner.rs            # Spinner animations
│   │   ├── transitions.rs        # State transitions
│   │   └── streaming.rs          # Streaming text effects
│   └── events.rs                 # UI event types

aura-cli/                         # CLI application (updated)
├── Cargo.toml
├── src/
│   ├── main.rs                   # Entry point, arg parsing
│   ├── session.rs                # Session management (existing)
│   ├── approval.rs               # Approval flow (existing)
│   ├── commands/
│   │   ├── mod.rs
│   │   └── parser.rs             # Slash command parsing
│   └── bridge.rs                 # Connects kernel events to UI
```

### 10.3 Dependencies

```toml
# aura-terminal/Cargo.toml

[package]
name = "aura-terminal"
version = "0.1.0"
edition = "2021"
description = "Cyber-retro terminal UI for AURA OS"
license = "MIT"

[dependencies]
# TUI Framework
ratatui = "0.28"              # Terminal UI framework
crossterm = "0.28"            # Cross-platform terminal control

# Async runtime
tokio = { version = "1.41", features = ["rt", "sync", "time"] }

# Syntax highlighting
syntect = "5.2"

# Serialization (for themes)
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"

# Time handling
chrono = { version = "0.4", features = ["serde"] }

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Shared types (minimal dependency)
aura-core = { path = "../aura-core" }

[dev-dependencies]
insta = "1.40"                # Snapshot testing
```

```toml
# aura-cli/Cargo.toml

[package]
name = "aura-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "aura"
path = "src/main.rs"

[dependencies]
# Terminal UI (our library!)
aura-terminal = { path = "../aura-terminal" }

# Async runtime
tokio = { version = "1.41", features = ["full"] }

# CLI argument parsing
clap = { version = "4.5", features = ["derive"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Internal crates
aura-core = { path = "../aura-core" }
aura-kernel = { path = "../aura-kernel" }
aura-reasoner = { path = "../aura-reasoner" }
aura-tools = { path = "../aura-tools" }
aura-store = { path = "../aura-store" }
```

### 10.4 Public API (`aura-terminal`)

```rust
// aura-terminal/src/lib.rs

//! # AURA Terminal
//! 
//! Cyber-retro terminal UI library for AURA OS.
//! 
//! ## Quick Start
//! 
//! ```rust,no_run
//! use aura_terminal::{Terminal, Theme, App};
//! 
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let theme = Theme::cyber();
//!     let mut terminal = Terminal::new(theme)?;
//!     let mut app = App::new();
//!     
//!     terminal.run(&mut app).await?;
//!     Ok(())
//! }
//! ```

pub mod components;
pub mod themes;
pub mod input;
pub mod layout;
pub mod animation;
pub mod events;

mod terminal;
mod app;
mod renderer;

// Re-exports for convenience
pub use terminal::Terminal;
pub use app::{App, AppState};
pub use themes::{Theme, ThemeConfig};
pub use events::{UiEvent, UiCommand};
pub use components::{
    Message, MessageRole,
    ToolCard, ToolStatus,
    DiffView, DiffLine,
    ApprovalModal, ApprovalChoice,
};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
```

### 10.5 Terminal Abstraction

```rust
// aura-terminal/src/terminal.rs

use crate::{App, Theme, layout::LayoutMode};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal as RatatuiTerminal,
    Frame,
};
use std::io::{self, Stdout};
use tokio::sync::mpsc;

pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    width: u16,
    height: u16,
}

impl Terminal {
    /// Create a new terminal with the given theme
    pub fn new(theme: Theme) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let inner = RatatuiTerminal::new(backend)?;
        let size = inner.size()?;
        
        Ok(Self {
            inner,
            theme,
            width: size.width,
            height: size.height,
        })
    }
    
    /// Get the current layout mode based on terminal width
    pub fn layout_mode(&self) -> LayoutMode {
        match self.width {
            0..=59 => LayoutMode::Compact,
            60..=79 => LayoutMode::Normal,
            80..=119 => LayoutMode::Comfortable,
            _ => LayoutMode::Wide,
        }
    }
    
    /// Get the current theme
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    
    /// Set a new theme
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }
    
    /// Run the main event loop
    pub async fn run(&mut self, app: &mut App) -> anyhow::Result<()> {
        loop {
            // Render current state
            self.inner.draw(|frame| {
                crate::renderer::render(frame, app, &self.theme);
            })?;
            
            // Handle events
            if crossterm::event::poll(std::time::Duration::from_millis(16))? {
                if let Event::Key(key) = crossterm::event::read()? {
                    if app.handle_key(key).should_quit() {
                        break;
                    }
                }
            }
            
            // Process any pending async updates
            app.tick().await;
        }
        Ok(())
    }
    
    /// Render a single frame (for testing or custom loops)
    pub fn render<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame, &Theme),
    {
        self.inner.draw(|frame| f(frame, &self.theme))?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Always restore terminal state on exit
        let _ = disable_raw_mode();
        let _ = execute!(self.inner.backend_mut(), LeaveAlternateScreen);
    }
}
```

### 10.6 Message Component

```rust
// aura-terminal/src/components/message.rs

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

pub struct MessageComponent {
    role: Role,
    content: Vec<ContentBlock>,
    timestamp: DateTime<Utc>,
    is_streaming: bool,
}

impl MessageComponent {
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (label, label_color, border_style) = match self.role {
            Role::User => ("YOU", theme.foreground, BorderType::Rounded),
            Role::Assistant => ("AURA", theme.primary, BorderType::Plain),
        };
        
        // Timestamp
        let timestamp = format!("[{}]", self.timestamp.format("%H:%M:%S"));
        let header = Line::from(vec![
            Span::styled(&timestamp, Style::default().fg(theme.muted)),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(label_color).add_modifier(Modifier::BOLD)),
        ]);
        
        // Streaming indicator
        let streaming_indicator = if self.is_streaming && self.role == Role::Assistant {
            " ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━▸"
        } else {
            ""
        };
        
        // Build content
        let mut lines = vec![header];
        
        for block in &self.content {
            match block {
                ContentBlock::Text { text } => {
                    lines.push(Line::from(text.as_str()));
                }
                ContentBlock::ToolUse { name, .. } => {
                    // Render tool card (delegate to ToolCardComponent)
                }
                ContentBlock::ToolResult { .. } => {
                    // Render tool output
                }
            }
        }
        
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_style)
            .border_style(Style::default().fg(label_color));
        
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        
        frame.render_widget(paragraph, area);
    }
}
```

### 10.7 Spinner Animation

```rust
// aura-terminal/src/animation/spinner.rs

use std::time::{Duration, Instant};

const SPINNER_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];
const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone, Copy, Default)]
pub enum SpinnerStyle {
    #[default]
    Dots,
    Braille,
    Wave,
    Box,
}

pub struct Spinner {
    frames: &'static [&'static str],
    current_frame: usize,
    last_update: Instant,
    interval: Duration,
}

impl Spinner {
    pub fn new(style: SpinnerStyle) -> Self {
        let frames = match style {
            SpinnerStyle::Dots => SPINNER_FRAMES,
            SpinnerStyle::Braille => BRAILLE_FRAMES,
            SpinnerStyle::Wave => &["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"],
            SpinnerStyle::Box => &["┤", "┘", "┴", "└", "├", "┌", "┬", "┐"],
        };
        
        Self {
            frames,
            current_frame: 0,
            last_update: Instant::now(),
            interval: Duration::from_millis(80),
        }
    }
    
    pub fn tick(&mut self) -> &str {
        if self.last_update.elapsed() >= self.interval {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
            self.last_update = Instant::now();
        }
        self.frames[self.current_frame]
    }
    
    pub fn reset(&mut self) {
        self.current_frame = 0;
        self.last_update = Instant::now();
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ProgressStyle {
    #[default]
    Blocks,
    Arrow,
    Gradient,
}

pub struct ProgressBar {
    progress: f32,  // 0.0 to 1.0
    style: ProgressStyle,
    width: u16,
}

impl ProgressBar {
    pub fn new(width: u16) -> Self {
        Self {
            progress: 0.0,
            style: ProgressStyle::default(),
            width,
        }
    }
    
    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }
    
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }
    
    pub fn render(&self) -> String {
        let filled = (self.progress * self.width as f32) as usize;
        let empty = self.width as usize - filled;
        
        match self.style {
            ProgressStyle::Blocks => {
                format!("[{}{}] {:.0}%", 
                    "█".repeat(filled),
                    "░".repeat(empty),
                    self.progress * 100.0
                )
            }
            ProgressStyle::Arrow => {
                format!("◇{}▸",
                    "━".repeat(filled.saturating_sub(1))
                )
            }
            ProgressStyle::Gradient => {
                format!("▓{}░",
                    "▓".repeat(filled.saturating_sub(1))
                )
            }
        }
    }
}
```

### 10.8 CLI Integration Example

```rust
// aura-cli/src/main.rs

use aura_terminal::{Terminal, Theme, App, UiEvent, UiCommand};
use aura_agent::AgentLoop;  // Was: aura_kernel::TurnProcessor
use aura_reasoner::AnthropicProvider;
use clap::Parser;
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(name = "aura", about = "AURA OS - AI Coding Assistant")]
struct Args {
    /// Theme to use (cyber, matrix, synthwave, minimal, light)
    #[arg(short, long, default_value = "cyber")]
    theme: String,
    
    /// Working directory
    #[arg(short, long)]
    dir: Option<PathBuf>,
    
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    
    // Load theme
    let theme = Theme::load(&args.theme)
        .unwrap_or_else(|_| Theme::cyber());
    
    // Create communication channels
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(100);
    
    // Initialize kernel components
    let provider = AnthropicProvider::from_env()?;
    let agent_loop = AgentLoop::new(config);  // Was: TurnProcessor::new(provider)
    
    // Create terminal app
    let mut app = App::new()
        .with_event_sender(ui_tx)
        .with_command_receiver(cmd_rx);
    
    if args.verbose {
        app.set_verbose(true);
    }
    
    // Create and run terminal
    let mut terminal = Terminal::new(theme)?;
    
    // Spawn kernel bridge task
    let bridge_handle = tokio::spawn(async move {
        crate::bridge::run(agent_loop, ui_rx, cmd_tx).await
    });
    
    // Run UI (blocking)
    terminal.run(&mut app).await?;
    
    // Cleanup
    bridge_handle.abort();
    
    Ok(())
}
```

```rust
// aura-cli/src/bridge.rs

//! Bridge between kernel events and terminal UI

use aura_terminal::{UiEvent, UiCommand, Message, MessageRole, ToolCard};
use aura_agent::AgentLoop;  // Was: aura_kernel::TurnProcessor
use tokio::sync::mpsc;

pub async fn run(
    mut agent_loop: AgentLoop,  // Was: TurnProcessor
    mut events: mpsc::Receiver<UiEvent>,
    commands: mpsc::Sender<UiCommand>,
) -> anyhow::Result<()> {
    while let Some(event) = events.recv().await {
        match event {
            UiEvent::UserMessage(text) => {
                // Send "thinking" status
                commands.send(UiCommand::SetStatus("Thinking...".into())).await?;
                
                // Process through kernel
                let mut stream = processor.process_prompt(&text).await?;
                
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        StreamChunk::Text(text) => {
                            commands.send(UiCommand::AppendText(text)).await?;
                        }
                        StreamChunk::ToolStart(name, id) => {
                            commands.send(UiCommand::ShowTool(ToolCard::new(name, id))).await?;
                        }
                        StreamChunk::ToolComplete(id, result) => {
                            commands.send(UiCommand::CompleteTool(id, result)).await?;
                        }
                        StreamChunk::Done => {
                            commands.send(UiCommand::SetStatus("Ready".into())).await?;
                        }
                    }
                }
            }
            UiEvent::Approve(tool_id) => {
                processor.approve_tool(&tool_id).await?;
            }
            UiEvent::Deny(tool_id) => {
                processor.deny_tool(&tool_id).await?;
            }
            UiEvent::Quit => break,
        }
    }
    
    Ok(())
}
```

---

## 11) User Flows

### 11.1 First Launch

```
User starts `aura` for the first time:

1. [100ms] Detect terminal capabilities (colors, size, Unicode)
2. [500ms] Boot sequence animation
3. Display welcome message with quick start guide
4. Show sample prompts user can try

╔═══════════════════════════════════════════════════════════════════════╗
║                                                                       ║
║     █████╗ ██╗   ██╗██████╗  █████╗      ██████╗ ███████╗            ║
║    ██╔══██╗██║   ██║██╔══██╗██╔══██╗    ██╔═══██╗██╔════╝            ║
║    ███████║██║   ██║██████╔╝███████║    ██║   ██║███████╗            ║
║    ██╔══██║██║   ██║██╔══██╗██╔══██║    ██║   ██║╚════██║            ║
║    ██║  ██║╚██████╔╝██║  ██║██║  ██║    ╚██████╔╝███████║            ║
║    ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝     ╚═════╝ ╚══════╝            ║
║                                                                       ║
║    ═══════════════════════════════════════════════════════════════    ║
║                                                                       ║
║    Welcome to AURA OS! I'm your AI coding assistant.                 ║
║                                                                       ║
║    Quick Start:                                                       ║
║    • Type a message to start chatting                                ║
║    • Use /help to see all commands                                   ║
║    • Press Tab for autocomplete                                      ║
║                                                                       ║
║    Try asking:                                                        ║
║    → "What files are in this project?"                               ║
║    → "Explain the main.rs file"                                      ║
║    → "Help me fix this bug..."                                       ║
║                                                                       ║
╚═══════════════════════════════════════════════════════════════════════╝

▸ _
```

### 11.2 Normal Interaction Flow

```
1. User types prompt
2. [instant] Input appears in chat
3. [instant] Status: "AURA is thinking..."
4. [streaming] AI response streams in character-by-character
5. [if tool_use] Tool card appears with spinner
6. [on tool complete] Checkmark, output displayed
7. [continue streaming] More response text
8. [on end_turn] Status: "Ready"
```

### 11.3 Approval Flow

```
1. AI requests tool that needs approval (e.g., fs.write)
2. Conversation pauses
3. Approval modal appears with highlighted border
4. Show: tool name, target file, preview of changes
5. User presses: Y (approve), N (deny), D (show diff)
6. If approved: execute and continue
7. If denied: AI receives denial, can try alternative
```

### 11.4 Error Recovery

```
1. Error occurs (network, tool failure, etc.)
2. Error toast appears (doesn't block UI)
3. Error details logged to conversation
4. Suggestions shown: /retry, /status, etc.
5. User can continue or investigate

╭────────────────────────────────────────────────────────────────────╮
│  ✗ Tool execution failed                                          │
│                                                                    │
│  fs.read: Permission denied for path '/etc/passwd'                │
│                                                                    │
│  This path is outside the allowed workspace.                      │
│  Only files within ~/projects/my-app/ are accessible.             │
│                                                                    │
│  Try: Ask for a file within the workspace                         │
╰────────────────────────────────────────────────────────────────────╯
```

---

## 12) Accessibility

### 12.1 Screen Reader Support

- All UI elements have text alternatives
- Status changes announced
- Avoid relying solely on color for meaning

### 12.2 Reduced Motion Mode

```toml
# ~/.aura/config.toml
[accessibility]
reduce_motion = true    # Disables animations
high_contrast = false   # Higher contrast colors
large_text = false      # Larger font (where supported)
```

When `reduce_motion = true`:
- Spinners show static indicator
- No typing animation
- Instant state transitions
- Progress bars update discretely

### 12.3 Color Blindness Considerations

All semantic states also have text/icon indicators:
- ✓ Success (not just green)
- ⚠ Warning (not just amber)
- ✗ Error (not just red)
- ● Active (not just color)

---

## 13) Testing

### 13.1 Visual Regression Tests

```rust
// aura-terminal/tests/snapshot_tests.rs

use aura_terminal::{
    components::{Message, MessageRole},
    themes::Theme,
    testing::TestTerminal,
};
use insta::assert_snapshot;

#[test]
fn test_message_rendering() {
    let mut term = TestTerminal::new(80, 24);
    let message = Message::new(MessageRole::Assistant, "Hello!");
    
    term.render(|f| message.render(f, f.area(), &Theme::cyber()));
    
    assert_snapshot!(term.to_string());
}

#[test]
fn test_tool_card_rendering() {
    let mut term = TestTerminal::new(80, 24);
    let card = ToolCard::new("fs.read", "tool_123")
        .with_status(ToolStatus::Complete)
        .with_result("234 bytes read");
    
    term.render(|f| card.render(f, f.area(), &Theme::cyber()));
    
    assert_snapshot!(term.to_string());
}
```

### 13.2 Interactive Tests

```rust
// aura-terminal/tests/interaction_tests.rs

use aura_terminal::{App, AppState, testing::TestTerminal};

#[tokio::test]
async fn test_approval_flow() {
    let mut app = App::new();
    
    // Simulate tool requiring approval
    app.request_approval("fs.write", "src/main.rs", "new content");
    
    // Should be in approval state
    assert!(matches!(app.state(), AppState::AwaitingApproval { .. }));
    
    // Approve via keyboard
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));
    
    // Should have approved
    assert!(app.last_approval_choice().is_some());
    assert!(app.last_approval_choice().unwrap().approved);
}

#[test]
fn test_input_history() {
    let mut app = App::new();
    
    app.submit_input("first command");
    app.submit_input("second command");
    
    // Navigate history
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
    assert_eq!(app.current_input(), "second command");
    
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
    assert_eq!(app.current_input(), "first command");
}
```

### 13.3 Theme Tests

```rust
// aura-terminal/tests/theme_tests.rs

use aura_terminal::{themes::Theme, testing::TestTerminal, renderer};

#[test]
fn test_all_themes_render() {
    let themes = ["cyber", "matrix", "synthwave", "minimal", "light"];
    
    for theme_name in themes {
        let theme = Theme::load(theme_name).unwrap();
        let mut term = TestTerminal::new(80, 24);
        let app = App::new();
        
        // Render all components
        term.render(|f| renderer::render(f, &app, &theme));
        
        // Should not panic, should have content
        assert!(!term.buffer().is_empty(), "Theme {} rendered empty", theme_name);
    }
}

#[test]
fn test_theme_colors_valid() {
    let theme = Theme::cyber();
    
    // All colors should be valid
    assert!(theme.primary.is_valid());
    assert!(theme.secondary.is_valid());
    assert!(theme.success.is_valid());
    assert!(theme.warning.is_valid());
    assert!(theme.error.is_valid());
}
```

### 13.4 Layout Tests

```rust
// aura-terminal/tests/layout_tests.rs

use aura_terminal::{layout::LayoutMode, Terminal};

#[test]
fn test_responsive_breakpoints() {
    // Compact
    assert_eq!(LayoutMode::from_width(40), LayoutMode::Compact);
    assert_eq!(LayoutMode::from_width(59), LayoutMode::Compact);
    
    // Normal
    assert_eq!(LayoutMode::from_width(60), LayoutMode::Normal);
    assert_eq!(LayoutMode::from_width(79), LayoutMode::Normal);
    
    // Comfortable
    assert_eq!(LayoutMode::from_width(80), LayoutMode::Comfortable);
    assert_eq!(LayoutMode::from_width(119), LayoutMode::Comfortable);
    
    // Wide
    assert_eq!(LayoutMode::from_width(120), LayoutMode::Wide);
    assert_eq!(LayoutMode::from_width(200), LayoutMode::Wide);
}
```

---

## 14) Implementation Checklist

### Phase 0: Crate Setup
- [ ] Create `aura-terminal` crate directory
- [ ] Set up `Cargo.toml` with dependencies
- [ ] Create module structure (`lib.rs`, submodules)
- [ ] Add to workspace `Cargo.toml`
- [ ] Update `aura-cli` to depend on `aura-terminal`

### Phase 1: Core Terminal (`aura-terminal`)
- [ ] Set up `ratatui` + `crossterm`
- [ ] Implement `Terminal` struct
- [ ] Basic render loop
- [ ] Raw mode handling + cleanup
- [ ] Event channel system (`UiEvent`, `UiCommand`)

### Phase 2: Components (`aura-terminal`)
- [ ] Header component
- [ ] Message component (user + assistant)
- [ ] Tool card component
- [ ] Status bar component
- [ ] Input component
- [ ] Toast notifications
- [ ] Modal dialogs

### Phase 3: Theming (`aura-terminal`)
- [ ] `Theme` struct + trait
- [ ] Cyber theme (default)
- [ ] Matrix, Synthwave, Minimal themes
- [ ] Theme loading from TOML
- [ ] Runtime theme switching

### Phase 4: Input Handling (`aura-terminal`)
- [ ] Key event loop
- [ ] Input history
- [ ] Autocomplete engine
- [ ] Path completion
- [ ] Command completion

### Phase 5: Animations (`aura-terminal`)
- [ ] Spinner system (multiple styles)
- [ ] Progress bars
- [ ] Streaming text effect
- [ ] State transition animations

### Phase 6: Layout (`aura-terminal`)
- [ ] Responsive breakpoints
- [ ] Compact mode
- [ ] Normal mode
- [ ] Wide mode with panels

### Phase 7: CLI Integration (`aura-cli`)
- [ ] Update `main.rs` to use `aura-terminal`
- [ ] Implement bridge module
- [ ] Slash command parsing
- [ ] Connect kernel events to UI
- [ ] Approval flow wiring

### Phase 8: Polish
- [ ] Boot sequence animation
- [ ] Error toasts
- [ ] Accessibility features
- [ ] Focus mode
- [ ] Verbose/debug mode
- [ ] Presentation mode

---

## 15) Acceptance Criteria

### Architecture
- [ ] `aura-terminal` is a standalone library crate
- [ ] `aura-cli` uses `aura-terminal` as a dependency
- [ ] UI components are reusable and testable in isolation
- [ ] Clean separation between UI and kernel logic

### Must Have
- [ ] Cyber-retro aesthetic with neon colors
- [ ] Streaming AI responses
- [ ] Tool execution with visual feedback
- [ ] Approval modal for write operations
- [ ] Diff preview
- [ ] Slash commands work
- [ ] Keyboard shortcuts work
- [ ] Graceful terminal resize
- [ ] Clean exit (restore terminal state)

### Nice to Have
- [ ] Multiple themes
- [ ] Sound effects
- [ ] Session save/load
- [ ] Export to markdown
- [ ] Wide mode side panels
- [ ] Custom themes

### User Experience
- [ ] New user can start chatting in < 30 seconds
- [ ] No documentation needed for basic use
- [ ] Errors are clear and actionable
- [ ] Never leaves terminal in broken state

---

## 16) Summary

This spec introduces `aura-terminal` as a **standalone Rust crate** providing:

| Feature | Description |
|---------|-------------|
| **Cyber Aesthetic** | Neon colors, ASCII art, box-drawing UI |
| **Rich Components** | Messages, tool cards, diffs, modals |
| **Themes** | Cyber, Matrix, Synthwave, Minimal, Light |
| **Animations** | Spinners, progress bars, streaming text |
| **Responsive Layout** | Adapts from 40-char to 200-char terminals |
| **Input System** | History, autocomplete, slash commands |
| **Event Architecture** | Async channels for UI ↔ kernel communication |

The crate is consumed by `aura-cli` but can be embedded in any Rust application that wants the AURA terminal experience.
