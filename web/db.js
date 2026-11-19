// IndexedDB 书架存储。
//
// 书架列表与书籍字节分开存：`books` 只放元数据，翻书架时不必把每本书的
// 几 MB 字节一起读出来；`bookData` 按 id 取原始 EPUB/TXT。`progress` 每本
// 一条，`settings` 全局一条，`assets` 用来缓存远程字体等可重建资源。
//
// 隐私模式、存储被策略禁用或测试环境里没有 indexedDB 时，所有操作退化成
// 进程内 Map：功能照常可用，只是刷新页面后丢失。

const DB_NAME = "ebook-reader";
const DB_VERSION = 1;
const STORES = ["books", "bookData", "progress", "settings", "assets"];
const KEYS = { books: "id", bookData: "id", progress: "bookId", settings: "key", assets: "key" };

const memory = Object.fromEntries(STORES.map((name) => [name, new Map()]));
let dbPromise = null;
let dbBroken = false;

function hasIndexedDB() {
  return !dbBroken && typeof indexedDB !== "undefined" && indexedDB !== null;
}

function openDb() {
  if (!hasIndexedDB()) {
    return Promise.resolve(null);
  }
  if (dbPromise) {
    return dbPromise;
  }
  dbPromise = new Promise((resolve, reject) => {
    let settled = false;
    const finish = (value) => {
      if (!settled) {
        settled = true;
        resolve(value);
      }
    };
    const fail = (error) => {
      if (!settled) {
        settled = true;
        dbBroken = true;
        reject(error);
      }
    };
    let request;
    try {
      request = indexedDB.open(DB_NAME, DB_VERSION);
    } catch (error) {
      fail(error);
      return;
    }
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const name of STORES) {
        if (!db.objectStoreNames.contains(name)) {
          db.createObjectStore(name, { keyPath: KEYS[name] });
        }
      }
    };
    request.onsuccess = () => {
      const db = request.result;
      db.onversionchange = () => db.close();
      finish(db);
    };
    request.onerror = () => fail(request.error || new Error("indexeddb open failed"));
    request.onblocked = () => fail(new Error("indexeddb blocked"));
    // 某些浏览器的隐私模式会永远不回调 open()。超时后退回内存，
    // 不能把整个书架/打开流程挂在这里。
    setTimeout(() => fail(new Error("indexeddb timeout")), 3000);
  }).catch(() => null);
  return dbPromise;
}

function requestOf(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function transactionDone(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error || new Error("transaction aborted"));
  });
}

function memoryKey(store, value, key) {
  return key !== undefined ? key : value && value[KEYS[store]];
}

async function getRecord(store, key) {
  const db = await openDb();
  if (!db) {
    return memory[store].get(key) || null;
  }
  try {
    const tx = db.transaction(store, "readonly");
    return (await requestOf(tx.objectStore(store).get(key))) || null;
  } catch (error) {
    return memory[store].get(key) || null;
  }
}

async function putRecord(store, value, key) {
  const db = await openDb();
  if (!db) {
    memory[store].set(memoryKey(store, value, key), value);
    return value;
  }
  try {
    const tx = db.transaction(store, "readwrite");
    const done = transactionDone(tx);
    if (key === undefined) {
      tx.objectStore(store).put(value);
    } else {
      tx.objectStore(store).put(value, key);
    }
    await done;
    return value;
  } catch (error) {
    memory[store].set(memoryKey(store, value, key), value);
    return value;
  }
}

async function allRecords(store) {
  const db = await openDb();
  if (!db) {
    return [...memory[store].values()];
  }
  return new Promise((resolve) => {
    const out = [];
    let tx;
    try {
      tx = db.transaction(store, "readonly");
    } catch (error) {
      resolve([...memory[store].values()]);
      return;
    }
    const req = tx.objectStore(store).openCursor();
    req.onsuccess = () => {
      const cursor = req.result;
      if (!cursor) {
        resolve(out);
        return;
      }
      out.push(cursor.value);
      cursor.continue();
    };
    req.onerror = () => resolve(out.length ? out : [...memory[store].values()]);
  });
}

function asBuffer(bytes) {
  if (bytes instanceof ArrayBuffer) {
    return bytes;
  }
  if (ArrayBuffer.isView(bytes)) {
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  }
  throw new Error("book data is not a byte buffer");
}

// ---- 公开 API -------------------------------------------------------------

export async function listBooks() {
  return allRecords("books");
}

export async function getBook(id) {
  return getRecord("books", id);
}

export async function getBookData(id) {
  const record = await getRecord("bookData", id);
  return record ? record.data : null;
}

export async function putBook(meta, bytes) {
  const data = asBuffer(bytes);
  const db = await openDb();
  if (!db) {
    memory.books.set(meta.id, meta);
    memory.bookData.set(meta.id, { id: meta.id, data });
    return meta;
  }
  try {
    const tx = db.transaction(["books", "bookData"], "readwrite");
    const done = transactionDone(tx);
    tx.objectStore("books").put(meta);
    tx.objectStore("bookData").put({ id: meta.id, data });
    await done;
    return meta;
  } catch (error) {
    memory.books.set(meta.id, meta);
    memory.bookData.set(meta.id, { id: meta.id, data });
    return meta;
  }
}

export async function putBookMeta(meta) {
  return putRecord("books", meta);
}

export async function removeBook(id) {
  const db = await openDb();
  if (!db) {
    memory.books.delete(id);
    memory.bookData.delete(id);
    memory.progress.delete(id);
    return;
  }
  try {
    const tx = db.transaction(["books", "bookData", "progress"], "readwrite");
    const done = transactionDone(tx);
    tx.objectStore("books").delete(id);
    tx.objectStore("bookData").delete(id);
    tx.objectStore("progress").delete(id);
    await done;
  } catch (error) {
    memory.books.delete(id);
    memory.bookData.delete(id);
    memory.progress.delete(id);
  }
}

export async function listProgress() {
  return allRecords("progress");
}

export async function getProgress(bookId) {
  return getRecord("progress", bookId);
}

export async function putProgress(bookId, progress) {
  return putRecord("progress", { ...progress, bookId });
}

export async function getSetting(key) {
  const record = await getRecord("settings", key);
  return record ? record.value : null;
}

export async function putSetting(key, value) {
  return putRecord("settings", { key, value });
}

export async function getAsset(key) {
  const record = await getRecord("assets", key);
  return record ? record.value : null;
}

export async function putAsset(key, value) {
  return putRecord("assets", { key, value });
}

export async function clearLibrary() {
  const db = await openDb();
  if (!db) {
    for (const name of STORES) {
      memory[name].clear();
    }
    return;
  }
  for (const name of STORES) {
    const tx = db.transaction(name, "readwrite");
    tx.objectStore(name).clear();
  }
}

export const storage = {
  listBooks,
  getBook,
  getBookData,
  putBook,
  putBookMeta,
  removeBook,
  listProgress,
  getProgress,
  putProgress,
  getSetting,
  putSetting,
  getAsset,
  putAsset,
  clearLibrary,
};
