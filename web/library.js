// 书架视图。只读元数据与进度，不碰书籍字节；点开时才由 reader 去取数据。

import { formatBytes } from "./text.js";

function element(id) {
  return document.getElementById(id);
}

function hueOf(id) {
  let hue = 0;
  for (let i = 0; i < String(id).length; i += 1) {
    hue = (hue * 31 + String(id).charCodeAt(i)) % 360;
  }
  return hue;
}

function formatName(format) {
  return String(format || "epub").toUpperCase();
}

export function createLibrary({ store, onOpen, onImport, onError }) {
  const shelfEl = element("shelf");
  const emptyEl = element("library-empty");
  const noteEl = element("library-note");
  const countEl = element("library-count");
  const searchEl = element("search");
  const importEl = element("import");
  const fileEl = element("file");

  let books = [];
  let progress = new Map();
  let filter = "";

  function showError(message) {
    if (onError) {
      onError(message);
    }
  }

  function render() {
    const query = filter.trim().toLowerCase();
    const shown = books
      .filter((book) => {
        if (!query) {
          return true;
        }
        return `${book.title || ""} ${book.author || ""} ${book.name || ""}`
          .toLowerCase()
          .includes(query);
      })
      .sort((a, b) => (b.updated || b.added || 0) - (a.updated || a.added || 0));

    shelfEl.replaceChildren();
    if (countEl) {
      countEl.textContent = books.length ? `共 ${books.length} 本` : "";
    }
    if (!shown.length) {
      if (emptyEl) {
        emptyEl.hidden = books.length > 0;
      }
      if (noteEl) {
        noteEl.hidden = books.length === 0;
      }
      return;
    }
    if (emptyEl) {
      emptyEl.hidden = true;
    }
    if (noteEl) {
      noteEl.hidden = true;
    }
    for (const book of shown) {
      shelfEl.append(renderCard(book, progress.get(book.id)));
    }
  }

  function renderCard(book, saved) {
    const card = document.createElement("article");
    card.className = "book-card";
    card.tabIndex = 0;

    const cover = document.createElement("div");
    cover.className = "book-cover";
    const hue = hueOf(book.id);
    cover.style.background = `linear-gradient(145deg, hsl(${hue} 38% 42%), hsl(${(hue + 48) % 360} 44% 28%))`;
    const badge = document.createElement("span");
    badge.className = "book-format";
    badge.textContent = formatName(book.format);
    const coverTitle = document.createElement("span");
    coverTitle.className = "book-cover-title";
    coverTitle.textContent = book.title || "Untitled";
    cover.append(badge, coverTitle);

    const info = document.createElement("div");
    info.className = "book-info";
    const title = document.createElement("h3");
    title.textContent = book.title || "Untitled";
    const meta = document.createElement("p");
    const parts = [];
    if (book.author) {
      parts.push(book.author);
    }
    parts.push(formatName(book.format));
    if (book.size) {
      parts.push(formatBytes(book.size));
    }
    meta.textContent = parts.join(" · ");
    info.append(title, meta);

    if (saved && typeof saved.percent === "number") {
      const bar = document.createElement("div");
      bar.className = "book-progress";
      const fill = document.createElement("span");
      fill.style.width = `${Math.round(Math.max(0, Math.min(1, saved.percent)) * 100)}%`;
      bar.append(fill);
      info.append(bar);
    }

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "book-delete";
    remove.textContent = "×";
    remove.title = `删除《${book.title || "Untitled"}》`;
    if (typeof remove.setAttribute === "function") {
      remove.setAttribute("aria-label", `删除《${book.title || "Untitled"}》`);
    }
    remove.addEventListener("click", async (event) => {
      if (event.stopPropagation) {
        event.stopPropagation();
      }
      if (typeof window.confirm === "function" && !window.confirm(`删除《${book.title || "Untitled"}》？`)) {
        return;
      }
      try {
        await store.removeBook(book.id);
        books = books.filter((item) => item.id !== book.id);
        progress.delete(book.id);
        render();
      } catch (error) {
        showError(`删除失败：${error.message || error}`);
      }
    });

    card.append(cover, info, remove);
    card.addEventListener("click", () => {
      onOpen(book);
    });
    card.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        if (event.preventDefault) {
          event.preventDefault();
        }
        onOpen(book);
      }
    });
    return card;
  }

  async function refresh() {
    try {
      const [records, progressRecords] = await Promise.all([
        store.listBooks(),
        store.listProgress ? store.listProgress() : Promise.resolve([]),
      ]);
      books = records || [];
      progress = new Map();
      for (const item of progressRecords || []) {
        progress.set(item.bookId, item);
      }
      render();
    } catch (error) {
      showError(`读取书架失败：${error.message || error}`);
    }
  }

  if (searchEl) {
    searchEl.addEventListener("input", () => {
      filter = searchEl.value || "";
      render();
    });
  }
  function requestImport() {
    if (onImport) {
      onImport();
    } else if (fileEl && typeof fileEl.click === "function") {
      fileEl.click();
    }
  }
  for (const button of [importEl, element("import-empty")]) {
    if (button) {
      button.addEventListener("click", requestImport);
    }
  }

  return {
    refresh,
    render,
    hideEmpty() {
      if (emptyEl) {
        emptyEl.hidden = true;
      }
    },
  };
}
