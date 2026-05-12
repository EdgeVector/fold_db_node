import { useCallback, useEffect, useRef, useState } from 'react'
import ForceGraph2D from 'react-force-graph-2d'
import type { Schema } from '../../types/schema'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas.js'
import { nativeIndexClient, mutationClient, schemaClient } from '../../api/clients'
import { getFieldNames, getSchemaDisplayName, toErrorMessage } from '../../utils/schemaUtils'
import { makeSchemaId, mergeGraphData, extractWordsFromRecord, buildFromResults, searchBatch } from '../../utils/graphUtils'
import type { GraphData, GraphNode, GraphLink, SearchResult, RecordLike } from '../../utils/graphUtils'
import NodeDetail from './graph/NodeDetail'

interface LoadStatus {
  phase: string
  progress: number
  total: number
}

type RenderNode = GraphNode & { x?: number; y?: number }

// Gruvbox-inspired palette
const COLORS = {
  schema:    '#83a598',
  word:      '#b8bb26',
  key:       '#fe8019',
  link:      '#504945',
  linkHover: '#928374',
  bg:        '#282828',
  text:      '#ebdbb2',
}

const MAX_WORDS     = 300  // cap on unique words to search
const MAX_RECORDS   = 20   // records to query per schema

export default function WordGraphTab() {
  const { approvedSchemas } = useApprovedSchemas() as { approvedSchemas: Schema[] }
  const [graphData, setGraphData] = useState<GraphData>({ nodes: [], links: [] })
  const [searchTerm, setSearchTerm] = useState('')
  const [isSearching, setIsSearching] = useState(false)
  const [loadStatus, setLoadStatus] = useState<LoadStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null)
  const [highlightNodes, setHighlightNodes] = useState<Set<string>>(new Set())
  const [highlightLinks, setHighlightLinks] = useState<Set<string>>(new Set())
  const graphRef = useRef<unknown>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)
  const [dimensions, setDimensions] = useState({ width: 800, height: 550 })
  const prepopulatedRef = useRef(false)

  useEffect(() => {
    if (!containerRef.current) return
    const ro = new ResizeObserver(entries => {
      for (const entry of entries) {
        setDimensions({ width: entry.contentRect.width, height: entry.contentRect.height })
      }
    })
    ro.observe(containerRef.current)
    return () => ro.disconnect()
  }, [])

  // Seed schema nodes whenever approved schemas change
  useEffect(() => {
    if (!approvedSchemas?.length) return
    const schemaNodes: GraphNode[] = approvedSchemas.map((s: Schema) => ({ id: makeSchemaId(s.name), label: getSchemaDisplayName(s), type: 'schema' as const }))
    setGraphData(prev => mergeGraphData(prev, schemaNodes, []))
  }, [approvedSchemas])

  const addResults = useCallback((results: SearchResult[]) => {
    const { nodes, links } = buildFromResults(results)
    setGraphData(prev => mergeGraphData(prev, nodes, links))
  }, [])

  // Auto-populate on first schema load
  const prepopulate = useCallback(async (schemas: Schema[]) => {
    if (prepopulatedRef.current || !schemas?.length) return
    prepopulatedRef.current = true

    setError(null)
    const allWords = new Set<string>()

    try {
      // Phase 1: query records from each schema to extract real words
      setLoadStatus({ phase: 'Reading records…', progress: 0, total: schemas.length })
      for (let i = 0; i < schemas.length; i++) {
        const schema = schemas[i]
        setLoadStatus({ phase: `Reading ${getSchemaDisplayName(schema)}…`, progress: i, total: schemas.length })
        try {
          const fields = getFieldNames(schema)
          const res = await mutationClient.executeQuery({ schema_name: schema.name, fields })
          const data = res.data as { results?: RecordLike[] } | undefined
          const records = Array.isArray(data?.results) ? data!.results! : []
          for (const record of records.slice(0, MAX_RECORDS)) {
            for (const w of extractWordsFromRecord(record)) {
              if (allWords.size < MAX_WORDS) allWords.add(w)
            }
          }
        } catch {
          // schema query failure is non-fatal
        }
      }

      if (allWords.size === 0) {
        // Fallback: list keys and use their hashes as seed terms
        for (const schema of schemas) {
          if (allWords.size >= MAX_WORDS) break
          try {
            const res = await schemaClient.listSchemaKeys(schema.name, 0, 50)
            const keys = (res.data as { keys?: Array<{ hash?: string; range?: string }> } | undefined)?.keys ?? []
            for (const kv of keys) {
              if (kv.hash && allWords.size < MAX_WORDS) allWords.add(kv.hash)
              if (kv.range && allWords.size < MAX_WORDS) allWords.add(kv.range)
            }
          } catch { /* non-fatal */ }
        }
      }

      if (allWords.size === 0) return

      // Phase 2: search each word in the native index
      const wordList = Array.from(allWords)
      let done = 0
      setLoadStatus({ phase: 'Indexing words…', progress: 0, total: wordList.length })
      await searchBatch(
        wordList,
        nativeIndexClient as unknown as Parameters<typeof searchBatch>[1],
        (results) => { addResults(results) },
        () => {
          done += 1
          setLoadStatus({ phase: 'Indexing words…', progress: done, total: wordList.length })
        }
      )
    } finally {
      setLoadStatus(null)
    }
  }, [addResults])

  useEffect(() => {
    if (approvedSchemas?.length) {
      prepopulate(approvedSchemas)
    }
  }, [approvedSchemas, prepopulate])

  const handleSearch = useCallback(async () => {
    const q = searchTerm.trim()
    if (!q) return
    setIsSearching(true)
    setError(null)
    try {
      const res = await nativeIndexClient.search(q)
      if (res.success) {
        const data = res.data as SearchResult[] | { results?: SearchResult[] } | undefined
        const results = (Array.isArray(data) ? data : data?.results) ?? []
        addResults(results)
        if (results.length === 0) setError(`No index entries for "${q}"`)
      } else {
        setError(res.error || 'Search failed')
      }
    } catch (e) {
      setError(toErrorMessage(e) || 'Search failed')
    } finally {
      setIsSearching(false)
    }
  }, [searchTerm, addResults])

  const handleNodeHover = useCallback((node: GraphNode | null) => {
    if (!node) { setHighlightNodes(new Set()); setHighlightLinks(new Set()); return }
    const hl = new Set<string>([node.id])
    const hlLinks = new Set<string>()
    for (const l of graphData.links) {
      const linkSrc = l.source as unknown as GraphNode | string
      const linkTgt = l.target as unknown as GraphNode | string
      const src = typeof linkSrc === 'object' && linkSrc ? linkSrc.id : linkSrc
      const tgt = typeof linkTgt === 'object' && linkTgt ? linkTgt.id : linkTgt
      if (src === node.id || tgt === node.id) {
        hlLinks.add(l.id); hl.add(src); hl.add(tgt)
      }
    }
    setHighlightNodes(hl)
    setHighlightLinks(hlLinks)
  }, [graphData.links])

  const handleNodeClick = useCallback((node: GraphNode) => {
    setSelectedNode(prev => prev?.id === node.id ? null : node)
  }, [])

  const nodeCanvasObject = useCallback((node: RenderNode, ctx: CanvasRenderingContext2D, globalScale: number) => {
    const isHighlighted = highlightNodes.has(node.id)
    const isSelected = selectedNode?.id === node.id
    const isSchema = node.type === 'schema'
    const baseColor = isSchema ? COLORS.schema : COLORS.word
    const r = isSchema ? 8 : 5

    const x = node.x ?? 0
    const y = node.y ?? 0
    ctx.beginPath()
    if (isSchema) {
      ctx.rect(x - r, y - r, r * 2, r * 2)
    } else {
      ctx.arc(x, y, r, 0, 2 * Math.PI)
    }
    ctx.fillStyle = isHighlighted || isSelected ? baseColor : `${baseColor}99`
    ctx.fill()
    if (isSelected) { ctx.strokeStyle = COLORS.key; ctx.lineWidth = 2; ctx.stroke() }

    const fontSize = Math.max(10 / globalScale, isSchema ? 11 : 9)
    ctx.font = `${isSchema ? 'bold ' : ''}${fontSize}px monospace`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = isHighlighted || isSelected ? COLORS.text : `${COLORS.text}99`
    const lbl = node.label && node.label.length > 20 ? node.label.slice(0, 18) + '…' : (node.label ?? '')
    ctx.fillText(lbl, x, y + r + fontSize)
  }, [highlightNodes, selectedNode])

  const linkCanvasObject = useCallback((link: GraphLink, ctx: CanvasRenderingContext2D) => {
    const src = link.source as unknown as RenderNode
    const tgt = link.target as unknown as RenderNode
    if (src?.x === undefined || src?.y === undefined || tgt?.x === undefined || tgt?.y === undefined) return
    const sx = src.x, sy = src.y, tx = tgt.x, ty = tgt.y
    const isHighlighted = highlightLinks.has(link.id)
    ctx.beginPath()
    ctx.moveTo(sx, sy)
    ctx.lineTo(tx, ty)
    ctx.strokeStyle = isHighlighted ? COLORS.linkHover : COLORS.link
    ctx.lineWidth = isHighlighted ? 1.5 : 0.8
    ctx.stroke()
    if (isHighlighted) {
      const mx = (sx + tx) / 2
      const my = (sy + ty) / 2
      ctx.font = '8px monospace'
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.fillStyle = COLORS.key
      ctx.fillText(link.keyLabel ?? '', mx, my - 5)
    }
  }, [highlightLinks])

  const handleClear = () => {
    const schemaNodes: GraphNode[] = (approvedSchemas ?? []).map((s: Schema) => ({ id: makeSchemaId(s.name), label: getSchemaDisplayName(s), type: 'schema' as const }))
    setGraphData({ nodes: schemaNodes, links: [] })
    setSelectedNode(null)
    setHighlightNodes(new Set())
    setHighlightLinks(new Set())
    prepopulatedRef.current = false
    prepopulate(approvedSchemas)
  }

  const wordNodeCount   = graphData.nodes.filter(n => n.type === 'word').length
  const schemaNodeCount = graphData.nodes.filter(n => n.type === 'schema').length
  const isLoading = !!loadStatus

  return (
    <div className="flex gap-4" style={{ height: '600px' }}>
      {/* Sidebar */}
      <div className="w-56 flex-shrink-0 flex flex-col gap-3 overflow-y-auto">
        <div>
          <div className="text-xs uppercase tracking-widest text-tertiary mb-2">Search Word</div>
          <div className="flex flex-col gap-2">
            <input
              type="text"
              value={searchTerm}
              onChange={e => setSearchTerm(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleSearch()}
              placeholder="e.g. alice"
              className="input text-sm"
              disabled={isLoading}
            />
            <button
              onClick={handleSearch}
              disabled={isSearching || isLoading || !searchTerm.trim()}
              className="btn-primary text-sm"
            >
              {isSearching ? 'Searching…' : 'Add to Graph'}
            </button>
          </div>
        </div>

        {/* Load status */}
        {loadStatus && (
          <div className="border border-border p-2 text-xs space-y-1">
            <div className="text-secondary">{loadStatus.phase}</div>
            <div className="w-full bg-surface-secondary h-1.5 rounded-full overflow-hidden">
              <div
                className="h-full bg-[#83a598] transition-all duration-300"
                style={{ width: `${loadStatus.total ? (loadStatus.progress / loadStatus.total) * 100 : 0}%` }}
              />
            </div>
            <div className="text-tertiary">{loadStatus.progress} / {loadStatus.total}</div>
          </div>
        )}

        <div className="flex flex-col gap-1 text-xs text-secondary border border-border p-2">
          <div>Schemas: <span className="text-primary font-mono">{schemaNodeCount}</span></div>
          <div>Words: <span className="text-primary font-mono">{wordNodeCount}</span></div>
          <div>Links: <span className="text-primary font-mono">{graphData.links.length}</span></div>
        </div>

        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-2 text-xs text-secondary">
            <span className="inline-block w-3 h-3" style={{ background: COLORS.schema }} />
            Schema (square)
          </div>
          <div className="flex items-center gap-2 text-xs text-secondary">
            <span className="inline-block w-3 h-3 rounded-full" style={{ background: COLORS.word }} />
            Word (circle)
          </div>
          <div className="flex items-center gap-2 text-xs text-secondary">
            <span className="inline-block w-8 h-px" style={{ background: COLORS.key }} />
            Key (hover)
          </div>
        </div>

        <button
          onClick={handleClear}
          disabled={isLoading}
          className="btn-secondary text-xs"
        >
          Clear & Reload
        </button>

        {error && (
          <div className="text-xs text-gruvbox-red border border-gruvbox-red/30 p-2">
            {error}
          </div>
        )}

        {selectedNode && (
          <div className="border border-border p-2">
            <div className="text-xs uppercase tracking-widest text-tertiary mb-2">Selected</div>
            <NodeDetail node={selectedNode} links={graphData.links} nodes={graphData.nodes} />
          </div>
        )}
      </div>

      {/* Graph Canvas */}
      <div
        ref={containerRef}
        className="flex-1 border border-border overflow-hidden relative"
        style={{ background: COLORS.bg }}
      >
        {isLoading && (
          <div className="absolute inset-0 flex items-center justify-center z-10 pointer-events-none">
            <div className="text-xs text-[#928374] bg-[#282828]/80 px-3 py-1.5 border border-[#504945]">
              {loadStatus.phase}
            </div>
          </div>
        )}
        <ForceGraph2D
          ref={graphRef as never}
          width={dimensions.width}
          height={dimensions.height}
          graphData={graphData}
          nodeCanvasObject={nodeCanvasObject}
          nodeCanvasObjectMode={() => 'replace'}
          linkCanvasObject={linkCanvasObject}
          linkCanvasObjectMode={() => 'replace'}
          onNodeHover={handleNodeHover}
          onNodeClick={handleNodeClick}
          cooldownTicks={100}
          nodePointerAreaPaint={(node: RenderNode, color: string, ctx: CanvasRenderingContext2D) => {
            const r = node.type === 'schema' ? 10 : 7
            ctx.fillStyle = color
            const x = node.x ?? 0
            const y = node.y ?? 0
            if (node.type === 'schema') {
              ctx.fillRect(x - r, y - r, r * 2, r * 2)
            } else {
              ctx.beginPath(); ctx.arc(x, y, r, 0, 2 * Math.PI); ctx.fill()
            }
          }}
          d3AlphaDecay={0.02}
          d3VelocityDecay={0.3}
        />
      </div>
    </div>
  )
}
