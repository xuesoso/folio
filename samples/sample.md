# folio sample document

This is a multi-page sample used to exercise folio's rendering and page
navigation. The same content is provided in three parallel formats — Markdown,
LaTeX, and PDF — so you can compare how each input path looks. Page with `n` / `p`
(or `j` / `k` at the page edges), jump with `:`〈n〉`Enter`, and zoom with `+` / `-`.

## 1. Introduction

folio renders document pages as images directly in your terminal using the sixel
or kitty graphics protocols. It opens PDFs natively, converts Markdown to PDF on
the fly, and — when you opt in via the `latex_engine` setting — compiles LaTeX
with an installed TeX engine. Whatever the source, the same viewer provides zoom,
pan, dark mode, configurable sharpness, bookmarks, and resume.

Everything you read here is laid out by folio's small pure-Rust engine for the
Markdown path: CommonMark is parsed, lowered to a block tree, and typeset with the
standard PDF fonts, paginating automatically when a page fills up. The text is
intentionally long enough to span more than two pages so multi-page behaviour is
easy to try out.

## 2. Text formatting

Paragraphs may contain **bold**, *italic*, ***bold italic***, and inline `code`
such as `let x = 42;`. Adjacent emphasis like **bold**`code`*italic* flows on a
single line, and long paragraphs wrap to the page width with the rest of the
prose, breaking cleanly at word boundaries rather than mid-word.

Soft breaks in the source
are joined into the same paragraph, so you can keep your Markdown lines short for
version control without affecting the rendered output. Hard breaks and explicit
blank lines start new paragraphs, exactly as CommonMark prescribes.

## 3. Lists

Unordered lists, including nested items:

- The quick brown fox jumps over the lazy dog.
- A second item with a little more text so it wraps to demonstrate hanging
  indentation under the bullet rather than back to the margin.
  - A nested item.
  - Another nested item.
- A third top-level item.

Ordered lists:

1. Preheat the oven.
2. Mix the ingredients thoroughly.
3. Bake until golden, then let cool before serving.

## 4. Code

Fenced code blocks render in a monospace face:

```rust
fn main() {
    let pages = vec!["one", "two", "three"];
    for (i, name) in pages.iter().enumerate() {
        println!("page {}: {}", i + 1, name);
    }
}
```

## 5. A longer discussion

Terminal document viewers occupy a useful niche: they keep you in the keyboard-
driven, text-first environment where so much development happens, while still
showing real, faithfully rendered pages. Rather than dumping a lossy text
approximation of a document, folio rasterizes the genuine page and paints it with
a graphics protocol, so figures, fonts, and layout survive intact.

Because only the page under view is rendered on demand, memory and CPU stay
modest even for large documents, and an LRU cache plus predictive prefetch keep
navigation feeling immediate. The same machinery that makes PDFs responsive
applies to converted Markdown and LaTeX, since every input is normalised to PDF
before it reaches the renderer.

This uniformity is the point of the modular input layer. Adding a new input format
is a matter of teaching folio how to turn that format into PDF bytes; the viewer,
the cache, the themes, and the bookmark store do not need to know or care where
those bytes came from. Markdown was the first such format, LaTeX the second, and
the same seam is ready for whatever comes next.

## 6. Symbols and math

The Markdown path renders many non-Latin characters from an embedded fallback
font: Greek (α, β, γ, π, Σ), operators (∑, ∫, √2, ∞, ≈, ≤, ≥, ≠, ×, ÷, ±), and
arrows (→, ←, ⇒). LaTeX math written as `$E = mc^2$` is shown here as literal
text, because the Markdown path does not typeset math — open the LaTeX sample to
see the same expression set properly.

> Reading a document in the terminal feels surprisingly natural once the pages
> render crisply: you keep your hands on the keyboard, your windows tiled, and
> your focus unbroken.

## 7. Conclusion

If you can see this section, pagination is working and you have scrolled or paged
past the first page. Try toggling dark mode with `i`, adjusting sharpness with `[`
and `]`, dropping a bookmark with `m`, and reopening the file to confirm it
resumes where you left off.
