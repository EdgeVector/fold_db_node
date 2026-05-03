import { useState, useCallback } from 'react'
import type { DragEvent, ChangeEvent } from 'react'
import type { OperationResultPayload } from '../../types/api'
import { ingestionClient } from '../../api/clients/ingestionClient'

interface FileUploadTabProps {
  onResult: (result: OperationResultPayload | null) => void
}

function FileUploadTab({ onResult }: FileUploadTabProps) {
  const [isDragging, setIsDragging] = useState(false)
  const [selectedFile, setSelectedFile] = useState<File | null>(null)
  const [autoExecute, setAutoExecute] = useState(true)
  const [pubKey] = useState('default')
  const [isUploading, setIsUploading] = useState(false)

  const handleDragEnter = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault()
    e.stopPropagation()
    setIsDragging(true)
  }, [])

  const handleDragLeave = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault()
    e.stopPropagation()
    setIsDragging(false)
  }, [])

  const handleDragOver = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault()
    e.stopPropagation()
  }, [])

  const handleDrop = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault()
    e.stopPropagation()
    setIsDragging(false)

    const files = e.dataTransfer.files
    if (files && files.length > 0) {
      setSelectedFile(files[0])
    }
  }, [])

  const handleFileSelect = useCallback((e: ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files
    if (files && files.length > 0) {
      setSelectedFile(files[0])
    }
  }, [])

  const handleUpload = async () => {
    if (!selectedFile) {
      onResult({
        success: false,
        error: 'Please select a file to upload'
      })
      return
    }

    setIsUploading(true)
    onResult(null)

    try {
      const response = await ingestionClient.uploadFile(selectedFile, {
        autoExecute,
        pubKey,
      })

      const result = response.data as {
        success?: boolean
        schema_name?: string
        schema_used?: string
        new_schema_created?: boolean
        mutations_generated?: number
        mutations_executed?: number
        error?: string
      } | undefined

      if (result?.success) {
        onResult({
          success: true,
          data: {
            schema_used: result.schema_name || result.schema_used,
            new_schema_created: result.new_schema_created,
            mutations_generated: result.mutations_generated,
            mutations_executed: result.mutations_executed
          }
        })
      } else {
        onResult({
          success: false,
          error: result?.error || response.error || 'Failed to process file'
        })
      }
    } catch (error) {
      onResult({
        success: false,
        error: (error instanceof Error ? error.message : String(error)) || 'Failed to process file'
      })
    } finally {
      setIsUploading(false)
    }
  }

  const formatFileSize = (bytes: number) => {
    if (bytes === 0) return '0 Bytes'
    const k = 1024
    const sizes = ['Bytes', 'KB', 'MB', 'GB']
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i]
  }

  return (
    <div className="space-y-4">
      {isUploading && (
        <div className="flex items-center gap-3 text-gruvbox-blue">
          <span className="spinner" />
          <span>Processing file...</span>
        </div>
      )}

      <div
        className={`border-2 border-dashed p-8 text-center transition-colors ${
          isDragging ? 'border-primary bg-surface-secondary' : 'border-border bg-surface hover:border-primary'
        }`}
        onDragEnter={handleDragEnter}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
      >
        {selectedFile ? (
          <div className="space-y-2">
            <p className="font-medium">{selectedFile.name}</p>
            <p className="text-sm text-secondary">{formatFileSize(selectedFile.size)}</p>
            <button onClick={() => setSelectedFile(null)} className="text-sm text-gruvbox-red">
              Remove
            </button>
          </div>
        ) : (
          <div className="space-y-3">
            <p className="text-secondary">Drop file here or click to browse</p>
            <p className="text-xs text-tertiary">PDF, DOCX, TXT, CSV, JSON, XML</p>
            <input type="file" id="file-upload" className="hidden" onChange={handleFileSelect} />
            <label htmlFor="file-upload" className="btn-secondary inline-block cursor-pointer">
              Browse
            </label>
          </div>
        )}
      </div>

      <div className="flex items-center justify-between">
        <label className="flex items-center gap-2 text-sm cursor-pointer">
          <input type="checkbox" checked={autoExecute} onChange={(e) => setAutoExecute(e.target.checked)} className="checkbox" />
          <span className="text-secondary" title="Automatically save extracted data to your database after processing">Auto-save</span>
        </label>

        <button
          onClick={handleUpload}
          disabled={isUploading || !selectedFile}
          className="btn-primary btn-lg flex items-center gap-2"
        >
          {isUploading ? <><span className="spinner" />Processing...</> : '→ Upload'}
        </button>
      </div>
    </div>
  )
}

export default FileUploadTab
