# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

<p>
  <a href="../../README.md">English</a> |
  <a href="./README.zh-CN.md">简体中文</a> |
  <a href="./README.zh-TW.md">繁體中文</a> |
  <a href="./README.ja.md">日本語</a> |
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <strong>العربية</strong>
</p>

Codeg (Code Generation) هو مساحة عمل مؤسسية متعددة الوكلاء للبرمجة.
يوحّد وكلاء البرمجة المحليين بالذكاء الاصطناعي (Claude Code، Codex CLI، OpenCode، Gemini CLI،
OpenClaw، وغيرها) في تطبيق سطح مكتب وخدمة ويب — مما يتيح التطوير عن بُعد من أي متصفح — مع تجميع الجلسات، والتطوير المتوازي
عبر `git worktree`، وإدارة MCP/Skills، وسير عمل متكامل لـ Git/الملفات/الطرفية.

## الواجهة الرئيسية
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## عرض الجلسات كبلاطات
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> الحالة الحالية: `v0.2.x` (تكرار سريع، مناسب للمتبنين الأوائل)

## أبرز المزايا

- مساحة عمل موحّدة متعددة الوكلاء في نفس المشروع
- استيعاب محلي للجلسات مع عرض منظّم
- تطوير متوازي مع تدفقات `git worktree` مدمجة
- إدارة MCP (فحص محلي + بحث/تثبيت من السجل)
- إدارة Skills (نطاق عام ونطاق المشروع)
- إدارة حسابات Git البعيدة (GitHub وخوادم Git الأخرى)
- وضع خدمة الويب — الوصول إلى Codeg من أي متصفح للعمل عن بُعد
- حلقة هندسية متكاملة (شجرة الملفات، الفروقات، تغييرات git، الإيداع، الطرفية)

## النطاق المدعوم

### 1) استيعاب الجلسات (الجلسات التاريخية)

| الوكيل | مسار متغير البيئة | الافتراضي في macOS / Linux | الافتراضي في Windows |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> ملاحظة: متغيرات البيئة لها الأولوية على المسارات الافتراضية.

### 2) جلسات ACP في الوقت الفعلي

يدعم حاليًا 5 وكلاء: Claude Code وCodex CLI وGemini CLI وOpenCode وOpenClaw.

### 3) دعم إعدادات Skills

- مدعوم: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- سيتم إضافة المزيد من المحولات تدريجيًا

### 4) التطبيقات المستهدفة لـ MCP

الأهداف القابلة للكتابة حاليًا:

- Claude Code
- Codex
- OpenCode

## البدء السريع

### المتطلبات

- Node.js `>=22` (مُوصى به)
- pnpm `>=10`
- Rust stable (2021 edition)
- تبعيات بناء Tauri 2

مثال على Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### التطوير

```bash
pnpm install

# تطبيق سطح المكتب الكامل (Tauri + Next.js)
pnpm tauri dev

# الواجهة الأمامية فقط
pnpm dev

# تصدير ثابت للواجهة الأمامية إلى out/
pnpm build

# بناء تطبيق سطح المكتب
pnpm tauri build

# فحص الأكواد
pnpm eslint .

# فحوصات Rust (تنفيذ في src-tauri/)
cargo check
cargo clippy
cargo build
```

## الهندسة المعمارية

```text
Next.js 16 (Static Export) + React 19
        |
        | invoke()
        v
Tauri 2 Commands (Rust)
  |- ACP Manager
  |- Parsers (local session ingestion)
  |- Git / File Tree / Terminal runtime
  |- MCP marketplace + local config writer
  |- SeaORM + SQLite
        |
        v
Local Filesystem / Local Agent Data / Git Repos
```

## القيود

- الواجهة الأمامية تستخدم التصدير الثابت (`output: "export"`)
- لا توجد مسارات ديناميكية في Next.js (`[param]`)؛ استخدم معاملات الاستعلام بدلاً من ذلك
- معاملات أوامر Tauri: `camelCase` في الواجهة الأمامية، `snake_case` في Rust
- TypeScript في الوضع الصارم

## الخصوصية والأمان

- محلي أولاً بشكل افتراضي للتحليل والتخزين وعمليات المشروع
- الوصول إلى الشبكة يحدث فقط عند الإجراءات التي يبدأها المستخدم
- دعم بروكسي النظام لبيئات المؤسسات

## الترخيص

Apache-2.0. راجع `LICENSE`.
