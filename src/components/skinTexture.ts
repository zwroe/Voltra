export async function prepareSkinTexture(url: string): Promise<ImageData> {
  const image = new Image();
  image.src = url;
  await image.decode();
  if (image.width !== 64 || ![32, 64].includes(image.height)) throw new Error("Invalid Minecraft skin texture.");
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 64;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("Skin preview is unavailable.");
  ctx.drawImage(image, 0, 0);

  if (image.height === 32) {
    // Mirror each face of the right arm and leg into the legacy skin's missing left limbs.
    for (const [source, target] of [[0, 16], [40, 32]]) {
      for (const [x, y, offset] of [[4, 0, 4], [8, 0, 8], [0, 4, 8], [4, 4, 4], [8, 4, 0], [12, 4, 12]]) {
        const height = y === 0 ? 4 : 12;
        ctx.save();
        ctx.translate(target + offset + 4, 48 + y);
        ctx.scale(-1, 1);
        ctx.drawImage(image, source + x, 16 + y, 4, height, 0, 0, 4, height);
        ctx.restore();
      }
    }
    const hat = ctx.getImageData(32, 0, 32, 32);
    if (hat.data.every((alpha, index) => index % 4 !== 3 || alpha >= 128)) ctx.clearRect(32, 0, 32, 16);
  }

  // The game keeps the base opaque and leaves the outer layers transparent.
  for (const [x, y, width, height] of [[0, 0, 32, 16], [0, 16, 64, 16], [16, 48, 32, 16]]) {
    const region = ctx.getImageData(x, y, width, height);
    for (let alpha = 3; alpha < region.data.length; alpha += 4) region.data[alpha] = 255;
    ctx.putImageData(region, x, y);
  }
  return ctx.getImageData(0, 0, 64, 64);
}
