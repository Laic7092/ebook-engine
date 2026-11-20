// 选区交互：鼠标按下/拖动/抬起、触屏长按拖动 → 两次 hit → selection_rects →
// Canvas2D 半透明高亮；松开后写剪贴板。
//
// 这个模块不碰 IR，也不做坐标换算以外的排版：它只调用宿主给的 hit/select/linkAt，
// 三个回调背后是 wasm 的 eb_hit / eb_selection / eb_link_at。
//
// 画高亮用一个显式 draw()，由 app.js 在每次 render() 末尾调用，避免脏矩形和
// 反复截图；缩放/翻页时宿主调 clear()，旧坐标不会留下来。

const DRAG_THRESHOLD = 3;
const CANCEL_SLIDE = 8;
const LONG_PRESS_MS = 320;
const SWIPE_START = 12;
const SWIPE_MIN = 44;

function now() {
  return typeof performance !== "undefined" && performance.now ? performance.now() : Date.now();
}

export function createSelection(options) {
  const {
    canvas,
    page,
    hit,
    select,
    linkAt,
    onLink,
    onTap,
    onSwipe,
    onSwipeStart,
    onSwipeMove,
    onSwipeEnd,
    onSwipeCancel,
    setStatus,
    onChange,
  } = options;

  let anchor = null;
  let focus = null;
  let rects = [];
  let text = "";
  let pressed = false;
  let dragging = false;
  let moved = false;
  let swiping = false;
  let swipeBlocked = false;
  let swipeStart = 0;
  let startClient = { x: 0, y: 0 };
  let timer = 0;

  function clear() {
    anchor = null;
    focus = null;
    rects = [];
    text = "";
    pressed = false;
    dragging = false;
    moved = false;
    swiping = false;
    swipeBlocked = false;
    if (timer) {
      clearTimeout(timer);
      timer = 0;
    }
  }

  function geometry() {
    const state = page();
    if (!state) {
      return null;
    }
    return { state, box: canvas.getBoundingClientRect() };
  }

  // 客户端坐标 → 页内逻辑坐标（canvas 可能被 CSS 缩放，宽高比来自 IR 的页尺寸）。
  function toPage(event) {
    const geo = geometry();
    if (!geo) {
      return null;
    }
    const { state, box } = geo;
    if (!box.width || !box.height) {
      return null;
    }
    const info = state.info;
    const width = info.size.w + info.inset.left + info.inset.right;
    const height = info.size.h + info.inset.top + info.inset.bottom;
    return {
      page: state.index,
      x: ((event.clientX - box.left) / box.width) * width,
      y: ((event.clientY - box.top) / box.height) * height,
    };
  }

  function hitAt(point) {
    if (!point) {
      return null;
    }
    return hit(point.page, point.x, point.y);
  }

  function refresh() {
    if (!anchor || !focus) {
      return;
    }
    try {
      const result = select(anchor, focus);
      rects = result.rects;
      text = result.text;
    } catch (error) {
      rects = [];
      text = "";
      setStatus(`选区失败：${error.message || error}`);
    }
    if (onChange) {
      onChange();
    }
  }

  function begin(point) {
    anchor = hitAt(point);
    focus = anchor;
    rects = [];
    text = "";
    dragging = anchor !== null;
    moved = false;
    if (onChange) {
      onChange();
    }
  }

  async function copyText(value) {
    if (!value) {
      setStatus("选区为空");
      return;
    }
    try {
      if (typeof navigator !== "undefined" && navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(value);
        setStatus(`已复制 ${Array.from(value).length} 字`);
        return;
      }
    } catch (error) {
      // 权限被拒或不是安全上下文，落到下面的兜底。
    }
    try {
      const area = document.createElement("textarea");
      area.value = value;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.top = "-1000px";
      area.style.opacity = "0";
      document.body.append(area);
      area.select();
      const ok = typeof document.execCommand === "function" && document.execCommand("copy");
      area.remove();
      setStatus(ok ? `已复制 ${Array.from(value).length} 字` : "复制失败：请手动选择复制");
    } catch (error) {
      setStatus(`复制失败：${error.message || error}`);
    }
  }

  function handleLink(point) {
    if (!linkAt || !onLink) {
      return false;
    }
    let href = null;
    try {
      href = linkAt(point.page, point.x, point.y);
    } catch (error) {
      href = null;
    }
    if (href) {
      onLink(href);
      return true;
    }
    return false;
  }

  function tap(event, point) {
    if (!point) {
      return;
    }
    if (!handleLink(point) && onTap) {
      onTap(event);
    }
  }

  function end(event) {
    if (!pressed) {
      return;
    }
    pressed = false;
    if (timer) {
      clearTimeout(timer);
      timer = 0;
    }
    const point = toPage(event);
    if (dragging) {
      // 拖动结束：保留高亮，写剪贴板。
      if (moved && anchor && focus && (anchor.line !== focus.line || anchor.byte !== focus.byte)) {
        refresh();
        copyText(text);
      } else {
        // 没有真正拖动 → 当成点击（链接或宿主轻点）。
        tap(event, point);
        clear();
        if (onChange) {
          onChange();
        }
      }
      dragging = false;
      return;
    }
    // 触屏长按前就抬手 = 轻点：只处理链接或宿主轻点。
    tap(event, point);
    clear();
    if (onChange) {
      onChange();
    }
  }

  function pointerDown(event) {
    if (event.button !== undefined && event.button !== 0) {
      return;
    }
    pressed = true;
    startClient = { x: event.clientX, y: event.clientY };
    moved = false;
    const point = toPage(event);
    if (event.pointerType === "touch") {
      // 触屏：先等长按，长按前移动/抬手都算轻点；横向拖动则进入翻页手势。
      timer = setTimeout(() => {
        timer = 0;
        if (pressed) {
          begin(point);
        }
      }, LONG_PRESS_MS);
      if (canvas.setPointerCapture) {
        try {
          canvas.setPointerCapture(event.pointerId);
        } catch (error) {
          // 不支持的浏览器仍可收到 move/up。
        }
      }
      return;
    }
    begin(point);
    if (canvas.setPointerCapture) {
      try {
        canvas.setPointerCapture(event.pointerId);
      } catch (error) {
        // 某些浏览器对合成事件不支持指针捕获；拖动仍可用。
      }
    }
    event.preventDefault();
  }

  function pointerMove(event) {
    if (!pressed) {
      return;
    }
    const dx = event.clientX - startClient.x;
    const dy = event.clientY - startClient.y;
    if (!dragging) {
      if (swiping) {
        if (onSwipeMove) {
          onSwipeMove(dx, dy);
        }
        event.preventDefault();
        return;
      }
      if (swipeBlocked) {
        // 已经决定横向翻页，但宿主没有跟手动画：等抬手时再按阈值翻页。
        if (Math.abs(dx) < Math.abs(dy)) {
          pressed = false;
        }
        event.preventDefault();
        return;
      }
      if (timer) {
        const horizontal = Math.abs(dx) > Math.abs(dy) * 1.3;
        if (horizontal && Math.abs(dx) > SWIPE_START) {
          clearTimeout(timer);
          timer = 0;
          const direction = dx < 0 ? "next" : "prev";
          if (onSwipeStart) {
            if (onSwipeStart(direction)) {
              swiping = true;
              swipeStart = now();
              if (onSwipeMove) {
                onSwipeMove(dx, dy);
              }
            } else {
              swipeBlocked = true;
            }
          } else {
            // 没有跟手宿主：保持旧行为，越过阈值立即翻页。
            pressed = false;
            if (onSwipe) {
              onSwipe(direction);
            }
          }
          event.preventDefault();
          return;
        }
        if (Math.hypot(dx, dy) > CANCEL_SLIDE) {
          // 长按前就开始竖向滑：不算选区，交给宿主滚动/翻页。
          clearTimeout(timer);
          timer = 0;
          pressed = false;
        }
        return;
      }
      return;
    }
    if (!moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) {
      return;
    }
    moved = true;
    const point = toPage(event);
    const next = hitAt(point);
    if (!next) {
      return;
    }
    if (!focus || next.line !== focus.line || next.byte !== focus.byte) {
      focus = next;
      refresh();
    }
    event.preventDefault();
  }

  function pointerUp(event) {
    if (!pressed) {
      return;
    }
    if (swiping) {
      const dx = event.clientX - startClient.x;
      const dt = Math.max(1, now() - swipeStart);
      pressed = false;
      swiping = false;
      if (onSwipeEnd) {
        onSwipeEnd(dx, dt);
      }
      event.preventDefault();
      return;
    }
    if (swipeBlocked) {
      const dx = event.clientX - startClient.x;
      const dy = event.clientY - startClient.y;
      pressed = false;
      swipeBlocked = false;
      if (Math.abs(dx) >= SWIPE_MIN && Math.abs(dx) > Math.abs(dy) * 1.3 && onSwipe) {
        onSwipe(dx < 0 ? "next" : "prev");
      }
      if (timer) {
        clearTimeout(timer);
        timer = 0;
      }
      event.preventDefault();
      return;
    }
    end(event);
  }

  function pointerCancel(event) {
    if (swiping && onSwipeCancel) {
      onSwipeCancel();
    }
    clear();
    if (onChange) {
      onChange();
    }
    event.preventDefault();
  }

  canvas.addEventListener("pointerdown", pointerDown);
  canvas.addEventListener("pointermove", pointerMove);
  canvas.addEventListener("pointerup", pointerUp);
  canvas.addEventListener("pointercancel", pointerCancel);

  return {
    clear,
    /** 当前选区是否非空。 */
    active() {
      return rects.length > 0;
    },
    /** 选区原文（测试与宿主复制用）。 */
    text() {
      return text;
    },
    /** 在 render() 末尾调用：只画当前页的矩形。 */
    draw(ctx, pageIndex) {
      if (!rects.length) {
        return;
      }
      let painted = false;
      ctx.save();
      ctx.fillStyle = "rgba(64,120,220,0.28)";
      for (const item of rects) {
        if (item.page !== pageIndex) {
          continue;
        }
        ctx.fillRect(item.x, item.y, item.w, item.h);
        painted = true;
      }
      ctx.restore();
      return painted;
    },
  };
}
