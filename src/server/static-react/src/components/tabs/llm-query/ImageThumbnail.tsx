import { useEffect, useState } from 'react'

interface ImageThumbnailProps {
  fileHash: string
  sourceFile?: string | null
}

/** Fetches and displays an image from the file API */
export default function ImageThumbnail({ fileHash, sourceFile }: ImageThumbnailProps) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null)

  useEffect(() => {
    const url = `/api/file/${fileHash}?name=${encodeURIComponent(sourceFile || '')}`
    let revoked = false
    const userHash = localStorage.getItem('fold_user_hash')
    const headers: Record<string, string> = {}
    if (userHash) {
      headers['x-user-hash'] = userHash
      headers['x-user-id'] = userHash
    }
    fetch(url, { headers })
      .then((res) => { if (!res.ok) throw new Error(res.statusText); return res.blob() })
      .then((blob) => { if (!revoked) setBlobUrl(URL.createObjectURL(blob)) })
      .catch(() => {})
    return () => {
      revoked = true
      setBlobUrl((prev) => { if (prev) URL.revokeObjectURL(prev); return null })
    }
  }, [fileHash, sourceFile])

  if (!blobUrl) return null
  return (
    <img
      src={blobUrl}
      alt={sourceFile ?? ''}
      className="max-w-xs max-h-64 rounded border border-border object-contain bg-surface-secondary"
    />
  )
}
