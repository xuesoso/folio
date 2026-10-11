# Markdown Showcase: Math, Tables, and Formatting

*A sample document demonstrating LaTeX math, a large numeric table, and common Markdown sections.*

## Table of Contents

1. [Inline and Display Math](#1-inline-and-display-math)
2. [Large Table of Values](#2-large-table-of-values)
3. [Text Formatting](#3-text-formatting)
4. [Lists and Tasks](#4-lists-and-tasks)
5. [Code, Quotes, and Links](#5-code-quotes-and-links)

---

## 1. Inline and Display Math

Inline math looks like this: $E = mc^2$, or $a^2 + b^2 = c^2$.

The quadratic formula:

$$x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}$$

Euler's identity:

$$e^{i\pi} + 1 = 0$$

A Gaussian integral and a series:

$$\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}, \qquad \sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}$$

A matrix and a piecewise function:

$$
A = \begin{pmatrix} a & b \\ c & d \end{pmatrix}, \qquad
f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}
$$

Aligned equations (Maxwell):

$$
\begin{aligned}
\nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \\
\nabla \cdot \mathbf{B} &= 0 \\
\nabla \times \mathbf{E} &= -\frac{\partial \mathbf{B}}{\partial t} \\
\nabla \times \mathbf{B} &= \mu_0 \mathbf{J} + \mu_0 \varepsilon_0 \frac{\partial \mathbf{E}}{\partial t}
\end{aligned}
$$

## 2. Large Table of Values

Common functions evaluated at $x = n/2$. Numbers are right-aligned; parity is centered.

| n | x | x² | √x | eˣ | ln x | sin x | cos x | parity |
|--:|--:|--:|--:|--:|--:|--:|--:|:--:|
| 1 | 0.5 | 0.25 | 0.7071 | 1.6487 | -0.6931 | 0.4794 | 0.8776 | odd |
| 2 | 1.0 | 1.00 | 1.0000 | 2.7183 | 0.0000 | 0.8415 | 0.5403 | even |
| 3 | 1.5 | 2.25 | 1.2247 | 4.4817 | 0.4055 | 0.9975 | 0.0707 | odd |
| 4 | 2.0 | 4.00 | 1.4142 | 7.3891 | 0.6931 | 0.9093 | -0.4161 | even |
| 5 | 2.5 | 6.25 | 1.5811 | 12.1825 | 0.9163 | 0.5985 | -0.8011 | odd |
| 6 | 3.0 | 9.00 | 1.7321 | 20.0855 | 1.0986 | 0.1411 | -0.9900 | even |
| 7 | 3.5 | 12.25 | 1.8708 | 33.1155 | 1.2528 | -0.3508 | -0.9365 | odd |
| 8 | 4.0 | 16.00 | 2.0000 | 54.5982 | 1.3863 | -0.7568 | -0.6536 | even |
| 9 | 4.5 | 20.25 | 2.1213 | 90.0171 | 1.5041 | -0.9775 | -0.2108 | odd |
| 10 | 5.0 | 25.00 | 2.2361 | 148.4132 | 1.6094 | -0.9589 | 0.2837 | even |
| 11 | 5.5 | 30.25 | 2.3452 | 244.6919 | 1.7047 | -0.7055 | 0.7087 | odd |
| 12 | 6.0 | 36.00 | 2.4495 | 403.4288 | 1.7918 | -0.2794 | 0.9602 | even |
| 13 | 6.5 | 42.25 | 2.5495 | 665.1416 | 1.8718 | 0.2151 | 0.9766 | odd |
| 14 | 7.0 | 49.00 | 2.6458 | 1096.6332 | 1.9459 | 0.6570 | 0.7539 | even |
| 15 | 7.5 | 56.25 | 2.7386 | 1808.0424 | 2.0149 | 0.9380 | 0.3466 | odd |
| 16 | 8.0 | 64.00 | 2.8284 | 2980.9580 | 2.0794 | 0.9894 | -0.1455 | even |
| 17 | 8.5 | 72.25 | 2.9155 | 4914.7688 | 2.1401 | 0.7985 | -0.6020 | odd |
| 18 | 9.0 | 81.00 | 3.0000 | 8103.0839 | 2.1972 | 0.4121 | -0.9111 | even |
| 19 | 9.5 | 90.25 | 3.0822 | 13359.7268 | 2.2513 | -0.0752 | -0.9972 | odd |
| 20 | 10.0 | 100.00 | 3.1623 | 22026.4658 | 2.3026 | -0.5440 | -0.8391 | even |
| 21 | 10.5 | 110.25 | 3.2404 | 36315.5027 | 2.3514 | -0.8797 | -0.4755 | odd |
| 22 | 11.0 | 121.00 | 3.3166 | 59874.1417 | 2.3979 | -1.0000 | 0.0044 | even |
| 23 | 11.5 | 132.25 | 3.3912 | 98715.7710 | 2.4423 | -0.8755 | 0.4833 | odd |
| 24 | 12.0 | 144.00 | 3.4641 | 162754.7914 | 2.4849 | -0.5366 | 0.8439 | even |
| 25 | 12.5 | 156.25 | 3.5355 | 268337.2865 | 2.5257 | -0.0663 | 0.9978 | odd |
| 26 | 13.0 | 169.00 | 3.6056 | 442413.3920 | 2.5649 | 0.4202 | 0.9074 | even |
| 27 | 13.5 | 182.25 | 3.6742 | 729416.3698 | 2.6027 | 0.8038 | 0.5949 | odd |
| 28 | 14.0 | 196.00 | 3.7417 | 1202604.2842 | 2.6391 | 0.9906 | 0.1367 | even |
| 29 | 14.5 | 210.25 | 3.8079 | 1982759.2635 | 2.6741 | 0.9349 | -0.3549 | odd |
| 30 | 15.0 | 225.00 | 3.8730 | 3269017.3725 | 2.7081 | 0.6503 | -0.7597 | even |

### Small Summary Table

| Constant | Symbol | Value |
|:---------|:------:|------:|
| Pi | $\pi$ | 3.14159265 |
| Euler's number | $e$ | 2.71828183 |
| Golden ratio | $\varphi$ | 1.61803399 |
| Speed of light | $c$ | 299 792 458 m/s |

## 3. Text Formatting

**Bold**, *italic*, ***bold italic***, ~~strikethrough~~, `inline code`, and H~2~O / x^2^ (if supported).

### Third-level heading

#### Fourth-level heading

## 4. Lists and Tasks

- Unordered item
  - Nested item
    - Deeper item
- Another item

1. First
2. Second
   1. Sub-step
3. Third

- [x] Write the document
- [x] Add math
- [ ] Review rendering

## 5. Code, Quotes, and Links

```python
import math

def quadratic(a, b, c):
    d = b * b - 4 * a * c
    return [(-b + s * math.sqrt(d)) / (2 * a) for s in (1, -1)]
```

> "Mathematics is the language with which God has written the universe."
> — attributed to Galileo

Links: [Markdown Guide](https://www.markdownguide.org) and an image placeholder: `![alt text](image.png)`.

Footnote example.[^1]

[^1]: This is the footnote text.
