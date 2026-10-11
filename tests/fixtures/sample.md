# folio Markdown demo

This file exists so you can try Markdown rendering with `folio sample.md`. It is
also used by the test suite to exercise the Markdown → PDF path end to end.

## Inline formatting

A paragraph with **bold text**, *italic text*, ***both***, and some `inline
code`. Soft breaks
join onto the same line, and a [link](https://example.com) keeps its text.

## Lists

Unordered, with nesting:

- First item
- Second item
  - Nested item A
  - Nested item B
- Third item

Ordered:

1. Step one
2. Step two
3. Step three

## Code block

```rust
fn main() {
    println!("hello from folio");
}
```

## Block quote

> The terminal is a surprisingly nice place to read a document,
> especially when it can show real pages.

## Symbols

Non-Latin characters render from an embedded fallback font: Greek (α, β, π, θ, Σ),
math (∑, ∫, √2, ∞, ≈, ≤, ≥, ≠, ×, ÷, ±), and arrows (→, ←, ⇒).

## Math and tables

Inline math like $E = mc^2$ is typeset, and so is display math:

$$\int_0^1 x^2\,dx = \frac{1}{3}$$

| Feature | Status |
|:--------|:------:|
| Tables  | ✓ |
| Math    | ✓ |

---

That's the end of the demo.
