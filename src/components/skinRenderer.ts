type Point = [number, number, number];
type Texture = { width: number; data: Uint8ClampedArray };

export async function loadCapeTexture(url: string): Promise<ImageData> {
  const image = new Image();
  if (url.startsWith("https:")) image.crossOrigin = "anonymous";
  image.src = url;
  await image.decode();
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 32;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Cape preview is unavailable.");
  context.imageSmoothingEnabled = false;
  context.drawImage(image, 0, 0, 64, 32);
  return context.getImageData(0, 0, 64, 32);
}

export function drawSkin(canvas: HTMLCanvasElement, skin: Texture, angle: number, slim: boolean, legacy: boolean, cape?: Texture, idleTime?: number) {
  const context = canvas.getContext("2d");
  if (!context) return;
  const { width, height } = canvas;
  const frame = context.createImageData(width, height);
  const depth = new Float32Array(width * height).fill(-Infinity);
  const yaw = angle * Math.PI / 180;
  const cos = Math.cos(yaw);
  const sin = Math.sin(yaw);
  const scale = height / 36;
  const arm = slim ? 3 : 4;
  const boxes = [
    { x: -4, y: 0, w: 8, h: 8, d: 8, u: 0, v: 0, layer: [32, 0], inflate: .5 },
    { x: -4, y: 8, w: 8, h: 12, d: 4, u: 16, v: 16, layer: [16, 32], inflate: .25 },
    { x: -4 - arm, y: 8, w: arm, h: 12, d: 4, u: 40, v: 16, layer: [40, 32], inflate: .25 },
    { x: 4, y: 8, w: arm, h: 12, d: 4, u: 32, v: 48, layer: [48, 48], inflate: .25 },
    { x: -4, y: 20, w: 4, h: 12, d: 4, u: 0, v: 16, layer: [0, 32], inflate: .25 },
    { x: 0, y: 20, w: 4, h: 12, d: 4, u: 16, v: 48, layer: [0, 48], inflate: .25 },
  ];
  const motion = idleTime === undefined ? 0 : Math.sin(idleTime / 900);
  const headYaw = idleTime === undefined ? 0 : Math.sin(idleTime / 1400) * .07;
  const poses = [rotate("yaw", headYaw, 0), undefined, rotate("pitch", motion * .09, 10), rotate("pitch", -motion * .09, 10)];
  boxes.forEach(({ x, y, w, h, d, u, v }, index) => cuboid(skin, x, y, 0, w, h, d, u, v, 0, poses[index]));
  if (cape) cuboid(cape, -5, 8, -2.6, 10, 16, 1, 0, 0, 0, ([x, y, z]) => [x, y, z - (y - 8) * (.22 + motion * .04)]);
  boxes.forEach(({ x, y, w, h, d, layer: [u, v], inflate }, index) => {
    if (!legacy || index === 0) cuboid(skin, x, y, 0, w, h, d, u, v, inflate, poses[index]);
  });
  context.putImageData(frame, 0, 0);

  function project([x, y, z]: Point): Point {
    const rotated = z * cos - x * sin;
    const vertical = y - 16;
    return [width / 2 + (x * cos + z * sin) * scale,
      height / 2 + (vertical * .99 + rotated * .14) * scale,
      rotated * .99 - vertical * .14];
  }

  function rotate(axis: "yaw" | "pitch", radians: number, pivotY: number) {
    if (!radians) return undefined;
    const cos = Math.cos(radians), sin = Math.sin(radians);
    return ([x, y, z]: Point): Point => axis === "yaw"
      ? [x * cos + z * sin, y, z * cos - x * sin]
      : [x, pivotY + (y - pivotY) * cos - z * sin, (y - pivotY) * sin + z * cos];
  }

  function cuboid(texture: Texture, x: number, y: number, z: number, w: number, h: number, d: number,
    u: number, v: number, expand: number, pose?: (point: Point) => Point) {
    const x0 = x - expand, x1 = x + w + expand;
    const y0 = y - expand, y1 = y + h + expand;
    const z0 = z - d / 2 - expand, z1 = z + d / 2 + expand;
    const view = pose ? (point: Point) => project(pose(point)) : project;
    function face(a: Point, b: Point, c: Point, tx: number, ty: number, tw: number, th: number, shade: number) {
      renderFace(texture, view(a), view(b), view(c), tx, ty, tw, th, shade);
    }
    face([x0, y0, z1], [x1, y0, z1], [x0, y1, z1], u + d, v + d, w, h, 1);
    face([x1, y0, z0], [x0, y0, z0], [x1, y1, z0], u + w + 2 * d, v + d, w, h, .87);
    face([x0, y0, z0], [x0, y0, z1], [x0, y1, z0], u, v + d, d, h, .8);
    face([x1, y0, z1], [x1, y0, z0], [x1, y1, z1], u + w + d, v + d, d, h, .9);
    face([x0, y0, z0], [x1, y0, z0], [x0, y0, z1], u + d, v, w, d, 1);
    face([x0, y1, z1], [x1, y1, z1], [x0, y1, z0], u + d + w, v, w, d, .75);
  }

  function renderFace(texture: Texture, [ax, ay, az]: Point, [bx, by, bz]: Point, [cx, cy, cz]: Point,
    tx: number, ty: number, tw: number, th: number, shade: number) {
    const ux = bx - ax, uy = by - ay, vx = cx - ax, vy = cy - ay;
    const area = ux * vy - uy * vx;
    if (area < 1e-6) return;
    const dx = bx + vx, dy = by + vy;
    const left = Math.max(0, Math.floor(Math.min(ax, bx, cx, dx)));
    const right = Math.min(width, Math.ceil(Math.max(ax, bx, cx, dx)));
    const top = Math.max(0, Math.floor(Math.min(ay, by, cy, dy)));
    const bottom = Math.min(height, Math.ceil(Math.max(ay, by, cy, dy)));
    for (let y = top; y < bottom; y++) {
      for (let x = left; x < right; x++) {
        const px = x + .5 - ax, py = y + .5 - ay;
        const u = (px * vy - py * vx) / area;
        const v = (ux * py - uy * px) / area;
        if (u < -1e-6 || u > 1 + 1e-6 || v < -1e-6 || v > 1 + 1e-6) continue;
        const index = y * width + x;
        const distance = az + u * (bz - az) + v * (cz - az);
        if (distance < depth[index]) continue;
        const sx = tx + Math.min(tw - 1, Math.max(0, Math.floor(u * tw)));
        const sy = ty + Math.min(th - 1, Math.max(0, Math.floor(v * th)));
        const first = (sy * texture.width + sx) * 4;
        const alpha = texture.data[first + 3] / 255;
        if (alpha < .1) continue;
        const last = index * 4;
        const behind = frame.data[last + 3] / 255 * (1 - alpha);
        const opacity = alpha + behind;
        for (let channel = 0; channel < 3; channel++) {
          frame.data[last + channel] = (texture.data[first + channel] * shade * alpha + frame.data[last + channel] * behind) / opacity;
        }
        frame.data[last + 3] = opacity * 255;
        depth[index] = distance;
      }
    }
  }
}
