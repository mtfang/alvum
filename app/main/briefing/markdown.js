function createBriefingMarkdown({
  fs,
  path,
  BRIEFINGS_DIR,
  validDateStamp,
}) {
  let markdownRendererPromise = null;

  function markdownRenderer() {
    if (!markdownRendererPromise) {
      markdownRendererPromise = Promise.all([
        import('unified'),
        import('remark-parse'),
        import('remark-gfm'),
        import('remark-math'),
        import('remark-rehype'),
        import('rehype-sanitize'),
        import('rehype-katex'),
        import('rehype-stringify'),
      ]).then(([
        unifiedMod,
        remarkParseMod,
        remarkGfmMod,
        remarkMathMod,
        remarkRehypeMod,
        rehypeSanitizeMod,
        rehypeKatexMod,
        rehypeStringifyMod,
      ]) => unifiedMod.unified()
        .use(remarkParseMod.default)
        .use(remarkGfmMod.default)
        .use(remarkMathMod.default)
        .use(remarkRehypeMod.default)
        .use(rehypeSanitizeMod.default)
        .use(rehypeKatexMod.default, { strict: false, throwOnError: false })
        .use(rehypeStringifyMod.default));
    }
    return markdownRendererPromise;
  }

  async function renderBriefingMarkdown(markdown) {
    const processor = await markdownRenderer();
    const rendered = await processor.process(markdown || '');
    return String(rendered);
  }

  async function readBriefingForDate(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const file = path.join(BRIEFINGS_DIR, date, 'briefing.md');
    try {
      const stat = fs.statSync(file);
      const markdown = fs.readFileSync(file, 'utf8');
      return {
        ok: true,
        date,
        path: file,
        mtime: new Date(stat.mtimeMs).toLocaleString(),
        markdown,
        html: await renderBriefingMarkdown(markdown),
      };
    } catch (e) {
      return { ok: false, date, error: e.message };
    }
  }

  return {
    renderBriefingMarkdown,
    readBriefingForDate,
  };
}

module.exports = { createBriefingMarkdown };
