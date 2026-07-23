# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED)](../../Dockerfile)

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

Codeg (Code Generation) هو مساحة عمل للبرمجة متعددة الوكلاء. يجمع عدة وكلاء (Claude Code، Codex CLI، OpenCode، Gemini CLI، OpenClaw، Cline، Hermes Agent، CodeBuddy، Kimi Code، Pi، Grok Build، وغيرها) في مساحة عمل واحدة، ويدعم تجميع المحادثات والتعاون بين عدة وكلاء، مع دعم التثبيت على سطح المكتب والنشر على الخادم/Docker.

![gallery](../images/gallery.svg)

## الرعاة

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>شكراً لـ Compshare على رعاية هذا المشروع! Compshare هي منصة الذكاء الاصطناعي السحابية التابعة لشركة UCloud، وتقدّم باقات Plan للوكلاء بنماذج محلية بأسعار اقتصادية شهرياً أو حسب الاستخدام، بدءاً من 49 يوان/شهر. كما توفّر وصولاً مستقراً إلى النماذج الأجنبية عبر وكيل رسمي. تدعم التكامل مع Claude Code وCodex واستدعاءات API. جاهزة للمؤسسات: تزامن عالٍ، ودعم فني على مدار الساعة طوال أيام الأسبوع، وإصدار الفواتير ذاتياً. المستخدمون الذين يسجّلون عبر <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">هذا الرابط</a> يحصلون على رصيد تجريبي مجاني بقيمة 5 يوان على المنصة!</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE" target="_blank"><img src="../images/sui-xiang.jpg" alt="随想AI中转站" width="200" /></a><br/>
      <strong><a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">随想AI中转站</a></strong>
    </td>
    <td>شكراً لـ 随想AI中转站 على رعاية هذا المشروع! 随想AI中转站 هي مزوّد موثوق وفعّال لخدمات ترحيل واجهات API، وتوفّر خدمات الترحيل لنماذج Claude وCodex وGemini وغيرها. تحصل الحسابات الجديدة بعد <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">التسجيل</a> على رصيد تجريبي بقيمة 0.5 يوان مقابل تسجيل الحضور اليومي؛ وتُضاف عمليات الشحن بنسبة 1:1، دون اشتراك وبالدفع حسب الاستخدام. خطوط متعددة متكرّرة، وتعافٍ من الكوارث عبر المناطق، وتبديل تلقائي عند الأعطال — لتبقى اتصالات SSE طويلة الأمد دون انقطاع.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://hezu.ink/sign-up?aff=0wVz" target="_blank"><img src="../images/hezu-ink.jpg" alt="合租巴士" width="200" /></a><br/>
      <strong><a href="https://hezu.ink/sign-up?aff=0wVz">合租巴士</a></strong>
    </td>
    <td>شكراً لـ 合租巴士 على رعاية هذا المشروع! 合租巴士 هي منصة موثوقة وفعّالة لخدمات ترحيل الذكاء الاصطناعي، توفّر ترحيلاً عالي الاستقرار للنماذج الرئيسية مثل Codex وClaude Code. نسبة الشحن شفافة (1:1)، مع دعم لمعدّل Codex يبدأ من 0.08 فقط. <a href="https://hezu.ink/sign-up?aff=0wVz">انضم إلى المجموعة عبر الموقع الرسمي للحصول على رصيد تجريبي بقيمة 5 دولارات</a>.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta" target="_blank"><img src="../images/onehop.jpg" alt="OneHop" width="120" /></a><br/>
      <strong><a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">OneHop</a></strong>
    </td>
    <td>شكراً لـ OneHop على رعاية هذا المشروع! يمنح OneHop مستخدمي Codeg مفتاح API واحداً متوافقاً مع OpenAI للوصول إلى مئات النماذج الرائدة، بما في ذلك GPT وClaude وGemini وDeepSeek وKimi وQwen. بدّل بين النماذج دون إدارة حسابات مزوّدين متعددة أو تعديل التعليمات البرمجية مراراً وتكراراً، وادفع فقط مقابل ما تستخدمه. <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">سجّل عبر Codeg</a> للحصول على رصيد بقيمة 1 دولار، ثم انضم إلى مجتمع OneHop وشارك في نشاط الترحيب للحصول على 5 دولارات إضافية — بما يصل إلى 6 دولارات من الرصيد التجريبي إجمالاً.</td>
  </tr>
</table>

> هل ترغب في أن تصبح راعياً لـ Codeg؟ [راسلنا عبر البريد الإلكتروني.](mailto:itpkcn@gmail.com)

## الواجهة الرئيسية

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## التعاون متعدد الوكلاء

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## سير عمل المكتب

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## أبرز المزايا

- **تجميع المحادثات** — استيراد جلسات جميع الوكلاء المدعومين إلى مساحة عمل موحّدة
- **التعاون متعدد الوكلاء** — داخل جلسة واحدة، يفوّض الوكيل الرئيسي إلى وكلاء فرعيين من أنواع مختلفة (مثل Claude Code يستدعي Codex وGemini) لإنجاز مهمة بشكل مشترك، مع تشغيل كل وكيل فرعي كجلسة مستقلة
- تطوير متوازي مع تدفقات `git worktree` مدمجة
- **مُنشئ المشروع** — إنشاء مشاريع جديدة بصريًا مع معاينة حية
- **مستندات Office** — أنشئ وحلِّل وراجع وحرِّر ملفات .docx / .xlsx / .pptx عبر مجموعة أدوات officecli المدمجة؛ مع معاينة حية في تبويب الملف تُحدَّث فورًا أثناء تعديلات الوكيل
- **البحث العلمي** — مهارات علمية مدمجة (توليد الفرضيات، تصميم التجارب، الإحصاء، التمثيل المرئي، التقييم النقدي، البحث في الأدبيات) يمكن لأي وكيل استدعاؤها، وتُدار لكل وكيل
- **الأتمتة** — احفظ أي إعداد للمُحرِّر كمهمة أتمتة قابلة للإعادة تُنفَّذ بدون واجهة وفق جدول cron أو عند الطلب
- **قنوات الدردشة** — ربط Telegram وLark (Feishu) وiLink (Weixin) والمزيد بوكلاء البرمجة لاستقبال الإشعارات الفورية والتفاعل الكامل مع الجلسات والتحكم عن بُعد في المهام
- إدارة MCP (فحص محلي + بحث/تثبيت من السجل)
- إدارة Skills (نطاق عام ونطاق المشروع)
- إدارة حسابات Git البعيدة (GitHub وخوادم Git الأخرى)
- وضع خدمة الويب — الوصول إلى Codeg من أي متصفح للعمل عن بُعد
- **نشر خادم مستقل** — شغّل `codeg-server` على أي خادم Linux/macOS، والوصول عبر المتصفح
- **دعم Docker** — `docker compose up` أو `docker run`، مع رمز مصادقة ومنفذ قابلين للتخصيص، واستمرارية البيانات وتحميل مجلدات المشاريع
- سجلات وقت التشغيل — عارض سجلات في الوقت الفعلي مدمج مع دعم التصفية وضبط مستويات السجل لكل وحدة
- حلقة هندسية متكاملة (شجرة الملفات، الفروقات، تغييرات git، الإيداع، الطرفية)

## الوكلاء المدعومون

| الوكيل       | مسار متغير البيئة                     | الافتراضي في macOS / Linux            | الافتراضي في Windows                                  |
| ------------ | ------------------------------------- | ------------------------------------- | ----------------------------------------------------- |
| Claude Code  | `$CLAUDE_CONFIG_DIR/projects`         | `~/.claude/projects`                  | `%USERPROFILE%\\.claude\\projects`                    |
| Codex CLI    | `$CODEX_HOME/sessions`                | `~/.codex/sessions`                   | `%USERPROFILE%\\.codex\\sessions`                     |
| OpenCode     | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI   | `$GEMINI_CLI_HOME/.gemini`            | `~/.gemini`                           | `%USERPROFILE%\\.gemini`                              |
| OpenClaw     | —                                     | `~/.openclaw/agents`                  | `%USERPROFILE%\\.openclaw\\agents`                    |
| Cline        | `$CLINE_DIR`                          | `~/.cline/data/tasks`                 | `%USERPROFILE%\\.cline\\data\\tasks`                  |
| Hermes Agent | `$HERMES_HOME/state.db`               | `~/.hermes/state.db`                  | `%USERPROFILE%\\.hermes\\state.db`                    |
| CodeBuddy    | `$CODEBUDDY_CONFIG_DIR/projects`      | `~/.codebuddy/projects`               | `%USERPROFILE%\\.codebuddy\\projects`                 |
| Kimi Code    | `$KIMI_CODE_HOME/sessions`            | `~/.kimi-code/sessions`               | `%USERPROFILE%\\.kimi-code\\sessions`                 |
| Pi           | `$PI_CODING_AGENT_SESSION_DIR`        | `~/.pi/agent/sessions`                | `%USERPROFILE%\\.pi\\agent\\sessions`                 |
| Grok Build   | `$GROK_HOME/sessions`                 | `~/.grok/sessions`                    | `%USERPROFILE%\\.grok\\sessions`                      |
| Cursor       | `$CURSOR_CONFIG_DIR/chats`            | `~/.cursor/chats`                     | `%USERPROFILE%\\.cursor\\chats`                       |

> ملاحظة: متغيرات البيئة لها الأولوية على المسارات الافتراضية.

<details>
<summary><h2>مُنشئ المشروع</h2></summary>

أنشئ مشاريع جديدة بصريًا من خلال واجهة مقسّمة: التكوين على اليسار، والمعاينة الحية على اليمين.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### الميزات

- **تكوين بصري** — اختر النمط وسمة الألوان ومكتبة الأيقونات والخط ونصف قطر الحدود والمزيد من القوائم المنسدلة؛ تتحدث المعاينة فورًا
- **معاينة حية** — شاهد المظهر الذي اخترته مُصيَّرًا في الوقت الفعلي قبل إنشاء أي شيء
- **إنشاء بنقرة واحدة** — اضغط "إنشاء مشروع" ويقوم المُشغّل بتنفيذ `shadcn init` مع إعداداتك المسبقة وقالب الإطار (Next.js / Vite / React Router / Astro / Laravel) ومدير الحزم (pnpm / npm / yarn / bun)
- **اكتشاف مدير الحزم** — يتحقق تلقائيًا من مديري الحزم المثبتين ويعرض إصداراتهم
- **تكامل سلس** — يُفتح المشروع المُنشأ حديثًا مباشرة في مساحة عمل Codeg

يدعم حاليًا إنشاء مشاريع **shadcn/ui**، مع تصميم قائم على علامات التبويب جاهز لدعم المزيد من أنواع المشاريع في المستقبل.

</details>

<details>
<summary><h2>قنوات الدردشة</h2></summary>

اربط تطبيقات المراسلة المفضلة لديك — Telegram وLark (Feishu) وiLink (Weixin) والمزيد — بوكلاء البرمجة بالذكاء الاصطناعي. أنشئ مهامًا، وأرسل رسائل متابعة، ووافق على الأذونات، واستأنف الجلسات، وراقب النشاط من تطبيق الدردشة — واستقبل ردود الوكلاء الفورية مع تفاصيل استدعاءات الأدوات وطلبات الأذونات وملخصات الإنجاز دون الحاجة لفتح المتصفح.

يمكن للمجموعات الفائقة ذات المنتدى في Telegram استخدام [Telegram topic mode](../chat-channels/telegram-topic-mode.md) لربط كل topic بجلسة Codeg مستقلة.

### القنوات المدعومة

| القناة         | البروتوكول                  | الحالة |
| -------------- | --------------------------- | ------ |
| Telegram       | Bot API (HTTP long-polling) | مدمج   |
| Lark (Feishu)  | WebSocket + REST API        | مدمج   |
| iLink (Weixin) | WebSocket + REST API        | مدمج   |

> يُخطَّط لدعم المزيد من القنوات (Discord وSlack وDingTalk وغيرها) في الإصدارات المستقبلية.

</details>

<details>
<summary><h2>مستندات Office</h2></summary>

تعامَل مع ملفات Word وExcel وPowerPoint كجزء أصيل من سير العمل. تتيح مجموعة أدوات **officecli** المدمجة لوكلائك إنشاء وتحليل ومراجعة وتحرير مستندات .docx و.xlsx و.pptx — مع إمكانية معاينة النتائج مباشرةً داخل Codeg.

### الميزات

- **إنشاء وتحرير** — أنشئ مستندات جديدة أو عدِّل ملفات .docx / .xlsx / .pptx الموجودة، بما في ذلك المخططات والجداول والتنسيق
- **تحليل ومراجعة** — افحص بنية المستند، واكشف مشكلات التنسيق، وراجع المحتوى
- **معاينة حية** — افتح ملف .docx / .xlsx / .pptx في تبويب الملف ليُعرَض تلقائيًا ويتحدّث فورًا مع كل تعديل من الوكيل — مدعومًا بخادم `officecli watch` دائم التشغيل (مع بروكسي عكسي ومصادقة قائمة على القدرات في بيئات الويب والخادم)
- **الإجراءات السريعة** — تتضمن صفحة الترحيب تبويبات «البرمجة» و«Office» و«البحث العلمي» تُتيح بنقرة واحدة إدراج استدعاء المهارة المناسب ونموذج الأمر في المُحرِّر؛ المهارات غير المفعَّلة تظهر بشارة قفل وتوجّهك للتفعيل
- **إعدادات أدوات Office** — صفحة إعدادات مخصصة لتثبيت `officecli` وإدارة مهاراته عبر مصفوفة مهارة×وكيل: بدِّل أي زوج (مهارة، وكيل) وطبِّق التغييرات على دفعات

</details>

<details>
<summary><h2>البحث العلمي</h2></summary>

حوِّل أي وكيل إلى مساعد بحثي دقيق. يُضمِّن Codeg مجموعة منتقاة من **مهارات البحث العلمي** المرخّصة بموجب MIT — من توليد الأفكار إلى التحليل إلى الكتابة — تُثبَّت في مخزن المهارات المركزي المشترك وتُربَط بأي وكلاء تختارهم، تمامًا مثل مجموعتَي أدوات الخبراء وOffice.

### الميزات

- **مهارات منتقاة** — توليد الفرضيات، تصميم التجارب، القوة الإحصائية، التحليل الإحصائي، التحليل الاستكشافي للبيانات، التمثيل المرئي العلمي، التقييم النقدي، مراجعة الأقران، إدارة الاستشهادات، تقييم الباحثين، البحث عن الأوراق البحثية، والرسوم التخطيطية بالذكاء الاصطناعي
- **الإجراءات السريعة** — يُدرج تبويب «البحث العلمي» في صفحة الترحيب استدعاء المهارة المناسب مع نموذج أمر مُترجَم في المُحرِّر بنقرة واحدة
- **إعدادات البحث العلمي** — صفحة إعدادات مخصصة تدير المهارات عبر مصفوفة مهارة×وكيل، مع شارات تُشير إلى المهارات التي تحتاج مفتاح API أو بيئة Python

</details>

<details>
<summary><h2>الأتمتة</h2></summary>

احفظ أي إعداد للمُحرِّر — الوكيل والنموذج والأمر ومجلد العمل والخيارات — كـ**مهمة أتمتة** قابلة للإعادة تعمل دون فتح الواجهة.

### الميزات

- **اضبط مرة، استخدم دائمًا** — احفظ إعداد المُحرِّر الكامل كمهمة أتمتة مُسمَّاة
- **مجدوَلة أو عند الطلب** — شغِّلها وفق جدول cron أو افتحها يدويًا متى أردت
- **تنفيذ بلا واجهة** — تعمل مهام الأتمتة في الخلفية وتُنشئ جلسات حقيقية يمكن فتحها في مساحة العمل في أي وقت؛ وبعد الإطلاق تعود الواجهة تلقائيًا إلى مساحة العمل

</details>

<details>
<summary><h2>البدء السريع</h2></summary>

### المتطلبات

- Node.js `>=22` (مُوصى به)
- pnpm `>=10`
- Rust stable (2021 edition)
- تبعيات بناء Tauri 2 (وضع سطح المكتب فقط)

مثال على Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### الملفات التنفيذية

يوفّر Codeg ثلاثة ملفات تنفيذية بلغة Rust من workspace واحد:

| الملف التنفيذي | الدور                                                                                                                  | البناء                                                                       |
| -------------- | ---------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `codeg`        | تطبيق سطح المكتب Tauri (نافذة، شريط النظام، المُحدِّث)                                                                 | `pnpm tauri build` (إصدار) / `pnpm tauri dev` (تطوير)                        |
| `codeg-server` | خادم HTTP + WebSocket مستقل لعمليات النشر عبر المتصفح/بدون واجهة                                                       | `pnpm server:build` / `pnpm server:dev`                                      |
| `codeg-mcp`    | رفيق MCP عبر stdio يُشغَّل لكل جلسة، ويُتيح أداة `delegate_to_agent` لواجهات CLI للوكلاء (التعاون متعدد الوكلاء)        | `pnpm tauri:prepare-sidecars` (يُستدعى تلقائيًا من `tauri dev` / `tauri build`) |

يجب أن يكون `codeg-mcp` بجوار ملفه التنفيذي الأصلي وقت التشغيل — برامج التثبيت وصورة Docker ومُجمِّع sidecar الخاص بـ Tauri جميعها تضعه بجوار `codeg` / `codeg-server`. يمكن لعمليات البناء من المصدر والتخطيطات المخصّصة تجاوز البحث باستخدام متغير البيئة `CODEG_MCP_BIN=/مسار/مطلق/codeg-mcp`. في حال غياب الرفيق، يتم تخطّي التفويض (مع تسجيل تحذير واحد) وتستمر باقي جلسة الوكيل في العمل.

### التطوير

```bash
pnpm install

# الواجهة الأمامية فقط (خادم تطوير Next.js، بدون Rust)
pnpm dev

# تصدير ثابت للواجهة الأمامية إلى out/
pnpm build

# تطبيق سطح المكتب الكامل (Tauri + Next.js، يبني sidecar الخاص بـ codeg-mcp تلقائيًا)
pnpm tauri dev

# بناء إصدار سطح المكتب (يُضمِّن codeg-mcp بوصفه externalBin)
pnpm tauri build

# خادم مستقل (بدون Tauri/واجهة رسومية)
pnpm server:dev
pnpm server:build                  # ملف الإصدار التنفيذي ضمن src-tauri/target/release/codeg-server

# بناء رفيق codeg-mcp بشكل صريح (لثلاثية المضيف)
pnpm tauri:prepare-sidecars        # الناتج: src-tauri/binaries/codeg-mcp-<triple>

# تخطّي تحضير sidecar عند التكرار على الواجهة الأمامية ولا تحتاج إلى التفويض
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# فحص الأكواد
pnpm eslint .

# اختبارات الواجهة الأمامية (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# فحوصات Rust (تنفيذ في src-tauri/)
cargo check                                                     # سطح المكتب (الميزات الافتراضية)
cargo check --no-default-features --bin codeg-server            # وضع الخادم
cargo check --no-default-features --bin codeg-mcp               # رفيق MCP
cargo clippy --all-targets --features test-utils -- -D warnings

# اختبارات Rust
cargo test --features test-utils                                # سطح المكتب (يشمل التكامل)
cargo test --no-default-features --bin codeg-server --lib       # وضع الخادم
cargo insta review                                              # قبول تحديثات لقطات المُحلِّل
```

> نصيحة: عند توفّر بناء جديد لـ `codeg-mcp` ضمن `src-tauri/target/release/` وأردت توجيه `codeg-server` مُشغَّل يدويًا إليه دون إعادة التثبيت، صدِّر `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### نشر الخادم

يمكن تشغيل Codeg كخادم ويب مستقل بدون بيئة سطح مكتب.

#### الخيار 1: التثبيت بسطر واحد (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

تثبيت إصدار محدد أو في دليل مخصص:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

ثم التشغيل:

```bash
codeg-server
```

#### الخيار 2: التثبيت بسطر واحد (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

أو تثبيت إصدار محدد:

```powershell
.\install.ps1 -Version v0.5.2
```

#### الخيار 3: التنزيل من GitHub Releases

الملفات التنفيذية المُعدّة مسبقًا (مع موارد الويب المضمّنة) متاحة في صفحة [Releases](https://github.com/xintaofei/codeg/releases):

| المنصة      | الملف                              |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# مثال: التنزيل والاستخراج والتشغيل
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

> لعمليات النشر غير المُراقَبة، شغّله باستخدام `--supervise` حتى يُتراجَع تلقائيًا عن أي ترقية في المكان تفشل — راجع [التحديث في المكان](#التحديث-في-المكان).

#### الخيار 4: Docker

```bash
# باستخدام Docker Compose (مُوصى به)
docker compose up -d

# أو التشغيل مباشرة باستخدام Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# مع رمز مصادقة مخصص وتحميل مجلد المشروع
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

تستخدم صورة Docker بناءً متعدد المراحل (Node.js + Rust → بيئة تشغيل Debian خفيفة) وتتضمن `git` و`ssh` لعمليات المستودعات. يتم تخزين البيانات بشكل دائم في وحدة التخزين `/data`. يمكنك اختياريًا تحميل مجلدات المشاريع للوصول إلى المستودعات المحلية من داخل الحاوية.

#### الخيار 5: البناء من المصدر

```bash
pnpm install && pnpm build          # بناء الواجهة الأمامية
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # رفيق التفويض
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # يتم التقاط codeg-mcp بوصفه ملفًا شقيقًا
```

إذا احتفظت بالملفين التنفيذيين في دليلين منفصلين، فاضبط `CODEG_MCP_BIN=/مسار/مطلق/إلى/codeg-mcp` حتى يستطيع التشغيل العثور على الرفيق؛ بدون ذلك، يُعطَّل التفويض متعدد الوكلاء بصمت.

#### التحديث في المكان

يمكن للخادم تحديث نفسه من **الإعدادات ← تحديث البرنامج**: إذ يُنزّل الإصدار المُوقَّع الخاص بمنصّته، ويستبدل الملفات التنفيذية وموارد الويب على القرص، ثم يُعيد التشغيل — دون إعادة نشر يدوية. هذه الميزة متاحة على Linux/macOS فقط (مُعطَّلة على Windows). يُحتفَظ بالإصدار السابق كنسخة احتياطية، لذا تُتيح الشاشة نفسها إجراء **التراجع** للعودة إليه.

**شغّله تحت المُشرِف للتراجع التلقائي.** ابدأ الخادم المستقل باستخدام `--supervise` كي تُعاد العملية المُرقّاة حديثًا تلقائيًا إلى الإصدار السابق إذا فشلت في الإقلاع ضمن نافذة التجربة:

```bash
CODEG_STATIC_DIR=./web ./codeg-server --supervise
```

بدون `--supervise` لا يزال الخادم يُحدِّث نفسه في المكان (إذ يُعيد تنفيذ نفسه)، لكن الترقية تبقى بأفضل جهد ممكن: لا يوجد مُشرِف يتراجع تلقائيًا عن إصدار يعجز عن البدء. أما صورة Docker فتعمل أصلًا تحت المُشرِف.

**ترقيات Docker تُغيِّر الحاوية لا الصورة.** تُعيد الترقية في المكان كتابة الملفات التنفيذية وموارد الويب داخل الطبقة القابلة للكتابة في الحاوية قيد التشغيل، فتوجد في تلك الحاوية وحدها. تبقى وحدة التخزين `/data` ثابتة، لكن الملفات المُرقّاة **لا تبقى**: إعادة إنشاء الحاوية — عبر `docker compose up --force-recreate` أو `docker run` جديد أو إعادة الإنشاء بعد `docker pull` — تبدأ من الصورة مجددًا وتُسقط الترقية في المكان. (تنفيذ `docker pull` وحده يُحدِّث الصورة المحلية فقط؛ ولا يحدث أي تراجع حتى يُعاد إنشاء الحاوية.) لجعل الترقية دائمة، ابنِ أو اسحب صورة بالإصدار الجديد وأعد إنشاء الحاوية منها.

#### التكوين

متغيرات البيئة:

| المتغير                        | الافتراضي              | الوصف                                                                                                                                                                                                                                                                                                                                                                                                       |
| ------------------------------ | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | منفذ HTTP                                                                                                                                                                                                                                                                                                                                                                                                   |
| `CODEG_HOST`                   | `0.0.0.0`              | عنوان الربط                                                                                                                                                                                                                                                                                                                                                                                                 |
| `CODEG_TOKEN`                  | _(عشوائي)_             | رمز المصادقة (يُطبع في stderr عند البدء)                                                                                                                                                                                                                                                                                                                                                                    |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | دليل قاعدة بيانات SQLite (والجذر أيضاً لـ `uploads/` و `pets/`)                                                                                                                                                                                                                                                                                                                                             |
| `CODEG_STATIC_DIR`             | `./web` أو `./out`     | دليل التصدير الثابت لـ Next.js                                                                                                                                                                                                                                                                                                                                                                              |
| `CODEG_MCP_BIN`                | _(غير مُحدّد)_         | المسار المطلق لرفيق `codeg-mcp`. يتجاوز البحث الافتراضي (ملف شقيق للملف التنفيذي + `PATH`). استخدمه لعمليات البناء من المصدر أو التخطيطات المخصّصة التي يقع فيها الرفيق خارج دليل تثبيت الخادم.                                                                                                                                                                                                            |
| `CODEG_SKIP_SIDECAR`           | _(غير مُحدّد)_         | متغير راحة للواجهة الأمامية فقط لـ `pnpm tauri dev` / `pnpm tauri build` — عند `1` يتم تخطّي بناء sidecar الخاص بـ `codeg-mcp`. يُعطَّل التفويض في هذا البناء؛ ويجب ترك المتغير غير مُحدَّد للقطع الصالحة للشحن.                                                                                                                                                                                            |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(غير مُحدّد)_         | حدّ صارم لإجمالي البايتات المقيمة تحت `<data dir>/uploads/`. عدد بايتات عشري (مثلاً `10737418240` لـ 10 GiB). إذا كان غير مُحدّد أو `0` أو قيمة لا يمكن تحليلها فسيتم تعطيل الحدّ وطباعة سطر عند البدء حتى تكون الحالة مرئية. يُطبَّق الحدّ داخل عملية `codeg-server` واحدة — تحتاج عمليات النشر الموسَّعة أفقياً التي تتشارك حجم `uploads/` واحداً إلى تنسيق خارجي (قفل ملف، Redis، حصّة عبر بروكسي عكسي). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(غير مُحدّد)_         | عند كونه صحيحاً (`1` / `true` / `yes` / `on`)، يُلغي البدء برمز خروج 2 إذا كانت `CODEG_UPLOAD_MAX_TOTAL_BYTES` مضبوطة على قيمة لا يمكن تحليلها، بدلاً من المتابعة مع تحذير WARN. استخدم هذا حين تتطلب سياستك الأمنية أن «تكون الحصّة المُعدَّة فعّالة».                                                                                                                                                     |

</details>

<details>
<summary><h2>الهندسة المعمارية</h2></summary>

```text
Next.js 16 (Static Export) + React 19
        |
        | invoke() (desktop) / fetch() + WebSocket (web)
        v
  ┌─────────────────────────┐
  │   Transport Abstraction  │
  │  (Tauri IPC or HTTP/WS) │
  └─────────────────────────┘
        |
        v
┌─── Tauri Desktop ───┐    ┌─── codeg-server ───┐
│  Tauri 2 Commands    │    │  Axum HTTP + WS    │
│  (window management) │    │  (standalone mode)  │
└──────────┬───────────┘    └──────────┬──────────┘
           └──────────┬───────────────┘
                      v
            Shared Rust Core
              |- AppState
              |- ACP Manager
              |- Parsers (conversation ingestion)
              |- Chat Channels
              |- Git / File Tree / Terminal
              |- MCP marketplace + config
              |- Office Tools (officecli) + Automations
              |- SeaORM + SQLite
                      |
              ┌───────┼───────┐
              v       v       v
  Local Filesystem  Git   Chat Channels
    / Git Repos    Repos  (Telegram, Lark, iLink)
```

</details>

## الخصوصية والأمان

- محلي أولاً بشكل افتراضي للتحليل والتخزين وعمليات المشروع
- الوصول إلى الشبكة يحدث فقط عند الإجراءات التي يبدأها المستخدم
- دعم بروكسي النظام لبيئات المؤسسات
- وضع خدمة الويب يستخدم مصادقة قائمة على الرموز

## المجتمع

- امسح رمز QR أدناه للانضمام إلى مجموعة WeChat الخاصة بنا للنقاشات والملاحظات والتحديثات

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- شكراً لمجتمع [LinuxDO](https://linux.do) على دعمه

## شكر وتقدير

- [ACP](https://agentclientprotocol.com) — بروتوكول Agent Client (ACP) هو الأساس الذي يمكّن Codeg من الاتصال بعدة وكلاء
- [Superpowers](https://github.com/obra/superpowers) — يُشغِّل وحدة مهارات الخبراء في Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — يُشغِّل سير عمل مستندات Office في Codeg
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) — يُشغِّل مهارات البحث العلمي في Codeg (مجموعة فرعية مرخّصة بموجب MIT)

## الترخيص

Apache-2.0. راجع `LICENSE`.
