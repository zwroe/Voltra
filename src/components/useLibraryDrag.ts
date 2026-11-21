import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent, type PointerEvent } from "react";

type LiftedTile = { id: string; width: number; height: number };
type Gesture = LiftedTile & {
  pointer: number;
  handle: HTMLElement;
  tile: HTMLElement;
  startX: number;
  startY: number;
  offsetX: number;
  offsetY: number;
  x: number;
  y: number;
  lifted: boolean;
};

const liftDistance = 6;
const transform = (x: number, y: number) => `translate3d(${x}px, ${y}px, 0)`;

function pointTarget(x: number, y: number) {
  const target = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-library-target]");
  return target?.dataset.libraryTarget && target.dataset.dropDisabled !== "true" ? target : null;
}

export function useLibraryDrag(animations: boolean, onDrop: (id: string, target: string) => void) {
  const gesture = useRef<Gesture | null>(null);
  const suppressClick = useRef<string | null>(null);
  const preview = useRef<HTMLDivElement>(null);
  const settleAnimation = useRef<Animation | null>(null);
  const [lifted, setLifted] = useState<LiftedTile | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const [landing, setLanding] = useState<string | null>(null);

  const position = useCallback((drag: Gesture) => {
    if (preview.current) preview.current.style.transform = transform(drag.x, drag.y);
  }, []);

  useLayoutEffect(() => {
    if (gesture.current && lifted) position(gesture.current);
  }, [lifted, position]);

  const finish = useCallback((target: HTMLElement | null = null) => {
    const drag = gesture.current;
    if (!drag) return;
    gesture.current = null;
    if (drag.handle.hasPointerCapture(drag.pointer)) drag.handle.releasePointerCapture(drag.pointer);
    if (!drag.lifted) return;
    setHover(null);
    const { id } = drag;
    const key = target?.dataset.libraryTarget;
    function commit() {
      setLifted(null);
      setLanding(null);
      if (key) onDrop(id, key);
    }

    const overlay = preview.current;
    if (!overlay || typeof overlay.animate !== "function" || !animations) {
      commit();
      return;
    }
    const rect = (target ?? drag.tile).getBoundingClientRect();
    if (!rect.width || !rect.height) {
      commit();
      return;
    }

    setLanding(key ?? null);
    const scale = target ? Math.min((rect.width - 10) / drag.width, (rect.height - 6) / drag.height, 0.76) : 1;
    const x = rect.left + (rect.width - drag.width) / 2;
    const y = rect.top + (rect.height - drag.height) / 2;
    const animation = overlay.animate([
      { transform: transform(drag.x, drag.y), opacity: 1 },
      { transform: `${transform(x, y)} scale(${Math.max(0.12, scale)})`, opacity: target ? 0 : 1 },
    ], { duration: target ? 205 : 175, easing: "cubic-bezier(.22,.78,.26,1)", fill: "forwards" });
    settleAnimation.current = animation;
    void animation.finished.then(() => {
      settleAnimation.current = null;
      commit();
    }, () => {});
  }, [animations, onDrop]);

  useEffect(() => () => {
    settleAnimation.current?.cancel();
    gesture.current = null;
  }, []);

  function pickUp(event: PointerEvent<HTMLButtonElement>, id: string, grip = false) {
    if (event.button !== 0 || !event.isPrimary || gesture.current || settleAnimation.current || (event.pointerType === "touch" && !grip)) return;
    suppressClick.current = null;
    const tile = event.currentTarget.closest<HTMLElement>("[data-instance-tile]");
    if (!tile) return;
    const handle = event.currentTarget;
    const rect = tile.getBoundingClientRect();
    gesture.current = {
      id, pointer: event.pointerId, handle, tile, width: rect.width, height: rect.height,
      startX: event.clientX, startY: event.clientY,
      offsetX: event.clientX - rect.left, offsetY: event.clientY - rect.top,
      x: rect.left, y: rect.top, lifted: false,
    };
    handle.setPointerCapture(event.pointerId);
  }

  useEffect(() => {
    function move(event: globalThis.PointerEvent) {
      const drag = gesture.current;
      if (!drag || drag.pointer !== event.pointerId) return;
      if (!drag.lifted) {
        if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < liftDistance) return;
        drag.lifted = true;
        suppressClick.current = drag.id;
        setLifted({ id: drag.id, width: drag.width, height: drag.height });
      }
      drag.x = event.clientX - drag.offsetX;
      drag.y = event.clientY - drag.offsetY;
      position(drag);
      setHover(pointTarget(event.clientX, event.clientY)?.dataset.libraryTarget ?? null);
    }
    const release = (event: globalThis.PointerEvent) => {
      if (gesture.current?.pointer === event.pointerId) finish(pointTarget(event.clientX, event.clientY));
    };
    const cancelPointer = (event: globalThis.PointerEvent) => {
      if (gesture.current?.pointer === event.pointerId) finish();
    };
    const cancel = () => finish();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && gesture.current?.lifted) {
        event.preventDefault();
        finish();
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", cancelPointer);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", cancelPointer);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", cancel);
    };
  }, [finish, position]);

  function consumeClick(event: MouseEvent<HTMLButtonElement>, id: string) {
    if (event.detail === 0 || suppressClick.current !== id) return false;
    suppressClick.current = null;
    event.preventDefault();
    return true;
  }

  function targetProps(key: string, disabled = false) {
    return {
      "data-library-target": key,
      "data-drop-disabled": disabled,
      "data-drop-active": hover === key,
      "data-drop-landing": landing === key,
    };
  }

  return { lifted, hover, preview, pickUp, targetProps, consumeClick };
}
