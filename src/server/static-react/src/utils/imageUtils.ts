const IMAGE_EXTENSIONS = /\.(jpe?g|png|gif|webp|svg)$/i

export interface ExtractedImage {
  fileHash: string
  sourceFile: string
}

/** Try to extract fileHash+sourceFile from a metadata-like object */
export function extractImageFromMeta(
  meta: unknown,
  seen: Set<string>,
  images: ExtractedImage[],
): void {
  if (!meta || typeof meta !== 'object') return
  const m = meta as { source_file_name?: string; file_hash?: string; metadata?: { file_hash?: string } }
  const sourceFile = m.source_file_name
  const fileHash = m.metadata?.file_hash || m.file_hash
  if (sourceFile && fileHash && IMAGE_EXTENSIONS.test(sourceFile) && !seen.has(fileHash)) {
    seen.add(fileHash)
    images.push({ fileHash, sourceFile })
  }
}

/** Extract unique image references from tool call results */
export function extractImagesFromToolCalls(toolCalls: unknown): ExtractedImage[] {
  const images: ExtractedImage[] = []
  const seen = new Set<string>()
  if (!Array.isArray(toolCalls)) return images
  for (const tc of toolCalls) {
    const tcObj = tc as { result?: unknown }
    const results = Array.isArray(tcObj.result) ? tcObj.result : []
    for (const record of results) {
      const metadata = (record as { metadata?: unknown } | null | undefined)?.metadata
      if (metadata && typeof metadata === 'object') {
        for (const val of Object.values(metadata)) {
          extractImageFromMeta(val, seen, images)
        }
        extractImageFromMeta(metadata, seen, images)
      }
    }
  }
  return images
}
