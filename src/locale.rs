// i18n module
use std::env;
use std::sync::LazyLock;

static CURRENT_LANG: LazyLock<&str> = LazyLock::new(|| {
    let from_config = crate::config::load_config().language;
    let lang = if !from_config.is_empty() {
        from_config
    } else {
        env::var("LANG").unwrap_or_default()
    };
    if lang.to_lowercase().starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    }
});

static LOCALE_EN: LazyLock<std::collections::HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = std::collections::HashMap::new();

    // Status icons
    m.insert("sess.think", "◉ Think");
    m.insert("sess.exec", "● Exec");
    m.insert("sess.wait", "◌ Wait");
    m.insert("sess.unknown", "? Unknown");
    m.insert("sess.done", "✓ Done");

    // Column headers
    m.insert("col.ai", "AI");
    m.insert("col.pid", "Pid");
    m.insert("col.project", "Project");
    m.insert("col.session", "Session");
    m.insert("col.sess", "Sess");
    m.insert("col.summary", "Summary");
    m.insert("col.status", "Status");
    m.insert("col.model", "Model");
    m.insert("col.context", "Context");
    m.insert("col.ctx", "Ctx");
    m.insert("col.tokens", "Tokens");
    m.insert("col.memory", "Memory");
    m.insert("col.turn", "Turn");

    // Sessions detail
    m.insert("detail.session", "SESSION");

    // Help panel
    m.insert("help.title", " Keybindings ");
    m.insert("help.navigation", "Navigation");
    m.insert("help.actions", "Actions");
    m.insert("help.views", "Views");
    m.insert("help.help", "Help");
    m.insert("help.press_key", " Press any key to close ");
    m.insert("help.select_session", "select session");
    m.insert("help.jump_tmux", "jump to session terminal");
    m.insert("help.filter", "filter sessions");
    m.insert("help.clear_filter", "clear filter / close overlay");
    m.insert("help.kill_session", "kill selected session");
    m.insert("help.kill_orphans", "kill orphan ports");
    m.insert("help.refresh", "force refresh");
    m.insert("help.quit", "quit");
    m.insert("help.view_menu", "open view menu");
    m.insert("help.open_config", "open config");
    m.insert("help.cycle_theme", "cycle theme");
    m.insert(
        "help.toggle_panels",
        "toggle panels (context/tokens/projects/ports/sessions)",
    );
    m.insert("help.this_help", "this help");

    // Footer
    m.insert("footer.select", "select");
    m.insert("footer.kill", "kill");
    m.insert("footer.filter", "filter");
    m.insert("footer.view", "view");
    m.insert("footer.config", "config");
    m.insert("footer.help", "help");
    m.insert("footer.quit", "quit");
    m.insert("footer.sessions", "sessions");
    m.insert("footer.auto", "auto");
    m.insert("footer.esc_clear", "Esc clear, Enter keep");
    m.insert("footer.jump", "jump");

    // View menu
    m.insert("view.title", " View ");
    m.insert("view.on", "on");
    m.insert("view.off", "off");
    m.insert("view.action", "→");
    m.insert("view.context_panel", "context panel");
    m.insert("view.tokens_panel", "tokens panel");
    m.insert("view.projects_panel", "projects panel");
    m.insert("view.ports_panel", "ports panel");
    m.insert("view.sessions_panel", "sessions panel");
    m.insert("view.cycle_theme", "cycle theme");
    m.insert("view.key_toggle", "key = toggle  ·  Esc = close ");

    // Header
    m.insert("header.cpu", "CPU");
    m.insert("header.mem", "MEM");
    m.insert("header.load", "L");
    m.insert("header.agents", "agents");
    m.insert("header.ctx", "ctx");

    // Tokens panel
    m.insert("tokens.title", "Total Tokens / all live sessions");
    m.insert("tokens.title_short", "Total Tokens / live");
    m.insert("tokens.total", "Total");
    m.insert("tokens.input", "Input");
    m.insert("tokens.output", "Output");
    m.insert("tokens.cache_r", "CacheR");
    m.insert("tokens.cache_w", "CacheW");
    m.insert("tokens.turns", "Turns");
    m.insert("tokens.avg", "Avg");
    m.insert("tokens.tokens_turn", "tokens/turn");
    m.insert("tokens.live", "Live");
    m.insert("tokens.complete", "Complete");
    m.insert("tokens.partial", "Partial");
    m.insert("tokens.unavailable", "Unavailable");

    // Context panel
    m.insert("context.rate", "Rate");
    m.insert("context.total", "Total");
    m.insert("context.active", "active");
    m.insert("context.project", "Project");
    m.insert("context.context", "Context");
    m.insert("context.window", "Window");
    m.insert("context.token_rate", "Token Rate");
    m.insert("context.no_active_sessions", "no active sessions");

    // Projects panel
    m.insert("projects.no_git", "no git");
    m.insert("projects.clean", "✓clean");
    m.insert("projects.no_projects", "no projects");

    // Ports panel
    m.insert("ports.port", "PORT");
    m.insert("ports.session", "SESSION");
    m.insert("ports.orphan", "orphan");
    m.insert("ports.no_open_ports", "no open ports");
    m.insert("ports.kill_orphans", "X to kill orphans");

    // Config panel
    m.insert("config.title", " Config ");
    m.insert("config.theme", "Theme");
    m.insert("config.on", "on");
    m.insert("config.off", "off");
    m.insert("config.change", "Enter/Space to change");
    m.insert("config.close", "Esc to close");
    m.insert("config.context_panel", "Context panel (1)");
    m.insert("config.tokens_panel", "Tokens panel (2)");
    m.insert("config.projects_panel", "Projects panel (3)");
    m.insert("config.ports_panel", "Ports panel (4)");
    m.insert("config.sessions_panel", "Sessions panel (5)");

    // Terminal size too small
    m.insert("term.too_small", "Terminal size too small:");
    m.insert("term.width", "Width");
    m.insert("term.height", "Height");
    m.insert("term.needed", "Needed for current config:");

    // Time formatting
    m.insert("time.s_ago", "s ago");
    m.insert("time.m_ago", "m ago");
    m.insert("time.h_ago", "h ago");
    m.insert("time.d_ago", "d ago");

    m
});

static LOCALE_ZH: LazyLock<std::collections::HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = std::collections::HashMap::new();

    // Status icons
    m.insert("sess.think", "◉ 思考");
    m.insert("sess.exec", "● 执行");
    m.insert("sess.wait", "◌ 等待");
    m.insert("sess.unknown", "? Unknown");
    m.insert("sess.done", "✓ 完成");

    // Column headers
    m.insert("col.ai", "AI");
    m.insert("col.pid", "PID");
    m.insert("col.project", "项目");
    m.insert("col.session", "会话");
    m.insert("col.sess", "会");
    m.insert("col.summary", "摘要");
    m.insert("col.status", "状态");
    m.insert("col.model", "模型");
    m.insert("col.context", "上下文");
    m.insert("col.ctx", "上");
    m.insert("col.tokens", "Token");
    m.insert("col.memory", "内存");
    m.insert("col.turn", "轮");

    // Sessions detail
    m.insert("detail.session", "会话");

    // Help panel
    m.insert("help.title", " 快捷键 ");
    m.insert("help.navigation", "导航");
    m.insert("help.actions", "操作");
    m.insert("help.views", "视图");
    m.insert("help.help", "帮助");
    m.insert("help.press_key", " 按任意键关闭 ");
    m.insert("help.select_session", "选择会话");
    m.insert("help.jump_tmux", "jump to session terminal");
    m.insert("help.filter", "过滤会话");
    m.insert("help.clear_filter", "清除过滤 / 关闭覆盖");
    m.insert("help.kill_session", "终止选中的会话");
    m.insert("help.kill_orphans", "终止孤立端口");
    m.insert("help.refresh", "强制刷新");
    m.insert("help.quit", "退出");
    m.insert("help.view_menu", "打开视图菜单");
    m.insert("help.open_config", "打开配置");
    m.insert("help.cycle_theme", "切换主题");
    m.insert(
        "help.toggle_panels",
        "切换面板 (上下文/词元/项目/端口/会话)",
    );
    m.insert("help.this_help", "显示帮助");

    // Footer
    m.insert("footer.select", "选择");
    m.insert("footer.kill", "终止");
    m.insert("footer.filter", "过滤");
    m.insert("footer.view", "视图");
    m.insert("footer.config", "配置");
    m.insert("footer.help", "帮助");
    m.insert("footer.quit", "退出");
    m.insert("footer.sessions", "会话");
    m.insert("footer.auto", "自动");
    m.insert("footer.esc_clear", "Esc 清除，Enter 保留");
    m.insert("footer.jump", "跳转");

    // View menu
    m.insert("view.title", " 视图 ");
    m.insert("view.on", "开");
    m.insert("view.off", "关");
    m.insert("view.action", "→");
    m.insert("view.context_panel", "上下文面板");
    m.insert("view.tokens_panel", "词元面板");
    m.insert("view.projects_panel", "项目面板");
    m.insert("view.ports_panel", "端口面板");
    m.insert("view.sessions_panel", "会话面板");
    m.insert("view.cycle_theme", "切换主题");
    m.insert("view.key_toggle", "按键切换  ·  Esc 关闭 ");

    // Header
    m.insert("header.cpu", "CPU");
    m.insert("header.mem", "内存");
    m.insert("header.load", "负载");
    m.insert("header.agents", "代理");
    m.insert("header.ctx", "上下文");

    // Tokens panel
    m.insert("tokens.title", "总 Token / 所有实时会话");
    m.insert("tokens.title_short", "总 Token / 实时");
    m.insert("tokens.total", "总计");
    m.insert("tokens.input", "输入");
    m.insert("tokens.output", "输出");
    m.insert("tokens.cache_r", "缓存读");
    m.insert("tokens.cache_w", "缓存写");
    m.insert("tokens.turns", "轮数");
    m.insert("tokens.avg", "平均");
    m.insert("tokens.tokens_turn", "词元/轮");
    m.insert("tokens.live", "实时");
    m.insert("tokens.complete", "完整");
    m.insert("tokens.partial", "部分");
    m.insert("tokens.unavailable", "不可用");

    // Context panel
    m.insert("context.rate", "速率");
    m.insert("context.total", "总计");
    m.insert("context.active", "活跃");
    m.insert("context.project", "项目");
    m.insert("context.context", "上下文");
    m.insert("context.window", "窗口");
    m.insert("context.token_rate", "Token 速率");
    m.insert("context.no_active_sessions", "无活跃会话");

    // Projects panel
    m.insert("projects.no_git", "非 Git");
    m.insert("projects.clean", "✓干净");
    m.insert("projects.no_projects", "无项目");

    // Ports panel
    m.insert("ports.port", "端口");
    m.insert("ports.session", "会话");
    m.insert("ports.orphan", "孤立");
    m.insert("ports.no_open_ports", "无开放端口");
    m.insert("ports.kill_orphans", "X 终止孤立");

    // Config panel
    m.insert("config.title", " 配置 ");
    m.insert("config.theme", "主题");
    m.insert("config.on", "开");
    m.insert("config.off", "关");
    m.insert("config.change", "Enter/空格 更改");
    m.insert("config.close", "Esc 关闭");
    m.insert("config.context_panel", "上下文面板 (1)");
    m.insert("config.tokens_panel", "词元面板 (2)");
    m.insert("config.projects_panel", "项目面板 (3)");
    m.insert("config.ports_panel", "端口面板 (4)");
    m.insert("config.sessions_panel", "会话面板 (5)");

    // Terminal size too small
    m.insert("term.too_small", "终端尺寸过小:");
    m.insert("term.width", "宽度");
    m.insert("term.height", "高度");
    m.insert("term.needed", "当前配置需要:");

    // Time formatting
    m.insert("time.s_ago", "秒前");
    m.insert("time.m_ago", "分前");
    m.insert("time.h_ago", "时前");
    m.insert("time.d_ago", "天前");

    m
});

pub fn t(key: &str) -> String {
    if *CURRENT_LANG == "zh-CN" {
        LOCALE_ZH
            .get(key)
            .map(|s| s.to_string())
            .unwrap_or_else(|| key.to_string())
    } else {
        LOCALE_EN
            .get(key)
            .map(|s| s.to_string())
            .unwrap_or_else(|| key.to_string())
    }
}
