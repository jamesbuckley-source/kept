/**
 * The release-notes page embedded by the update window.
 *
 * The page owns the layout and the wording of the examples; the window only hands
 * it the version and the markdown body of the release, so nothing here depends on
 * Kept's own components. It also opens on its own in a browser, so a missing
 * parent is normal and the shipped examples stay.
 */

const list = document.getElementById("features");
const versionChip = document.getElementById("version");
const heading = document.getElementById("heading");
const lede = document.getElementById("lede");

/** True when this document is a frame inside the update window. */
const embedded = window.parent !== window;

function post(message) {
  if (embedded) window.parent.postMessage(message, "*");
}

/** One line of markdown as text: emphasis and code are the whole vocabulary. */
function inline(text) {
  const fragment = document.createDocumentFragment();
  const pattern = /(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)]+\))/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    if (match.index > last) fragment.append(text.slice(last, match.index));
    const token = match[0];
    if (token.startsWith("**")) {
      const strong = document.createElement("strong");
      strong.textContent = token.slice(2, -2);
      fragment.append(strong);
    } else if (token.startsWith("`")) {
      const code = document.createElement("code");
      code.textContent = token.slice(1, -1);
      fragment.append(code);
    } else {
      const [, label, href] = /\[([^\]]+)\]\(([^)]+)\)/.exec(token);
      const link = document.createElement("a");
      link.textContent = label;
      link.href = href;
      link.dataset.external = "";
      link.style.color = "var(--accent-hi)";
      fragment.append(link);
    }
    last = match.index + token.length;
  }
  if (last < text.length) fragment.append(text.slice(last));
  return fragment;
}

/**
 * Splits a release body into cards: a heading starts one, and the lines under it
 * are its text and bullets. A body with no headings still gets a card per bullet,
 * which is what GitHub's auto-generated notes look like.
 */
function cards(markdown) {
  const found = [];
  let current = null;
  const start = (title) => {
    current = { title, lines: [] };
    found.push(current);
    return current;
  };

  for (const raw of markdown.replace(/\r\n/g, "\n").split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    // HTML comments are how a notes file is annotated for the people writing it;
    // they are not something to show on the page.
    if (line.startsWith("<!--")) continue;
    const headingMatch = /^#{1,4}\s+(.*)$/.exec(line);
    if (headingMatch) {
      start(headingMatch[1].replace(/^\[|\]$/g, "").trim());
      continue;
    }
    const bullet = /^[-*+]\s+(.*)$/.exec(line);
    if (!current) start("In this release");
    if (bullet) current.lines.push({ bullet: bullet[1] });
    else current.lines.push({ text: line });
  }
  return found.filter((card) => card.title && card.lines.length > 0);
}

function render(notes) {
  const parsed = cards(notes);
  if (parsed.length === 0) return;

  const built = document.createDocumentFragment();
  for (const card of parsed) {
    const item = document.createElement("li");
    item.className = "feature";

    const glyph = document.createElement("span");
    glyph.className = "glyph";
    glyph.textContent = "•";
    item.append(glyph);

    const body = document.createElement("div");
    const title = document.createElement("h3");
    title.textContent = card.title;
    body.append(title);

    const bullets = card.lines.filter((line) => line.bullet);
    for (const line of card.lines.filter((entry) => entry.text)) {
      const p = document.createElement("p");
      p.append(inline(line.text));
      body.append(p);
    }
    if (bullets.length > 0) {
      const ul = document.createElement("ul");
      for (const line of bullets) {
        const li = document.createElement("li");
        li.append(inline(line.bullet));
        ul.append(li);
      }
      body.append(ul);
    }

    item.append(body);
    built.append(item);
  }

  list.replaceChildren(built);
}

function showVersion(version) {
  if (!version) return;
  versionChip.textContent = `Version ${version}`;
  heading.textContent = `What's new in Kept ${version}`;
  lede.textContent =
    "Everything in this release, one card per feature. The full notes live with the release on GitHub.";
}

window.addEventListener("message", (event) => {
  const data = event.data;
  if (!data || data.type !== "kept:release-notes") return;
  showVersion(data.version);
  if (typeof data.notes === "string" && data.notes.trim()) {
    try {
      render(data.notes);
    } catch {
      // The examples that shipped with the page are a better fallback than a
      // half-drawn list.
    }
  }
});

// A link leaves the page through the host application, so the notes never turn
// into a web browser.
document.addEventListener("click", (event) => {
  const link = event.target instanceof Element ? event.target.closest("a[data-external]") : null;
  if (!link) return;
  if (!embedded) return;
  event.preventDefault();
  post({ type: "kept:open-url", url: link.href });
});

post({ type: "kept:ready" });
