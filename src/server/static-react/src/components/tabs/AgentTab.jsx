/**
 * AgentTab - Conversational AI interface for FoldDB
 *
 * Single unified input — the LLM decides what to do (query data, scan folders,
 * ingest files, etc.) using its available tools. No client-side branching.
 *
 * Phases:
 * 1. AI config - Inline provider setup if not configured
 * 2. Chat      - Everything goes through the agent query API
 */

import { useState, useCallback, useRef, useEffect } from 'react';
import { useAppSelector, useAppDispatch } from '../../store/hooks';
import {
  selectIsAiConfigured,
  selectAiProvider,
  selectActiveModel,
  fetchIngestionConfig,
  selectIngestionConfig,
} from '../../store/ingestionSlice';
import { selectApprovedSchemas, fetchSchemas } from '../../store/schemaSlice';
import { llmQueryClient } from '../../api/clients/llmQueryClient';
import { ingestionClient } from '../../api/clients/ingestionClient';
import { extractImagesFromToolCalls } from '../../utils/imageUtils';
import ImageThumbnail from './llm-query/ImageThumbnail';
import useAiConfig from '../settings/AiConfigSettings';

// State machine: determines UX based on system state
// loading → needs_ai → empty → has_data
function derivePhase(configLoaded, aiConfigured, schemas) {
  if (!configLoaded) return 'loading';
  if (!aiConfigured) return 'needs_ai';
  if (!schemas || schemas.length === 0) return 'empty';
  return 'has_data';
}

const PHASE_CONFIG = {
  empty: {
    welcome: "Welcome to FoldDB! I'm your AI assistant.\n\nI don't see any data yet. Let's fix that — tell me where your files are and I'll scan, classify, and ingest them for you.",
    placeholder: 'Tell me where your files are (e.g. "scan ~/Documents", "scan sample_data")...',
    heading: "Let's get some data in",
    suggestions: [
      { label: "Scan sample_data", text: "Scan the sample_data folder for files to ingest" },
      { label: "Scan ~/Documents", text: "Scan ~/Documents for personal files to ingest" },
      { label: "Scan ~/Desktop", text: "Scan ~/Desktop for files to ingest" },
      { label: "What can you do?", text: "What can you do?" },
    ],
  },
  has_data: {
    welcome: null, // no auto-message when data exists
    placeholder: 'Ask anything about your data, or scan a folder to add more...',
    heading: 'What would you like to know?',
    suggestions: [
      { label: "What data do I have?", text: "What data do I have? Give me a summary." },
      { label: "Recent entries", text: "Show me the most recent entries across all schemas" },
      { label: "Search my data", text: "Search for " },
      { label: "Add more data", text: "Scan ~/Documents for new files to ingest" },
    ],
  },
};

function AgentTab() {
  const dispatch = useAppDispatch();
  const aiConfigured = useAppSelector(selectIsAiConfigured);
  const aiProvider = useAppSelector(selectAiProvider);
  const activeModel = useAppSelector(selectActiveModel);
  const ingestionConfig = useAppSelector(selectIngestionConfig);
  const schemas = useAppSelector(selectApprovedSchemas);

  // Chat state
  const [messages, setMessages] = useState([]);
  const [inputText, setInputText] = useState('');
  const [isProcessing, setIsProcessing] = useState(false);
  const [sessionId, setSessionId] = useState(null);
  const conversationEndRef = useRef(null);
  const lastToolContextRef = useRef(null); // last scan/query result for attaching to next request

  // AI config inline form state
  const [configSaveStatus, setConfigSaveStatus] = useState(null);
  const aiConfig = useAiConfig({
    configSaveStatus,
    setConfigSaveStatus,
    onClose: () => dispatch(fetchIngestionConfig()),
  });

  // Progress polling state — tracks active ingestion during agent processing
  const [activeProgress, setActiveProgress] = useState(null);
  const progressPollRef = useRef(null);

  // Poll ingestion progress while the agent is processing
  useEffect(() => {
    if (!isProcessing) {
      if (progressPollRef.current) {
        clearInterval(progressPollRef.current);
        progressPollRef.current = null;
      }
      setActiveProgress(null);
      return;
    }

    const poll = async () => {
      try {
        const resp = await ingestionClient.getAllProgress();
        if (resp.success && Array.isArray(resp.data) && resp.data.length > 0) {
          // Find the most recently started non-complete agent job, or any active job
          const active = resp.data.find(j => !j.is_complete && !j.is_failed) || null;
          setActiveProgress(active);
        } else {
          setActiveProgress(null);
        }
      } catch {
        // ignore polling errors
      }
    };

    // Start polling after a short delay (agent needs time to begin ingestion)
    const timeout = setTimeout(() => {
      poll();
      progressPollRef.current = setInterval(poll, 2000);
    }, 1000);

    return () => {
      clearTimeout(timeout);
      if (progressPollRef.current) {
        clearInterval(progressPollRef.current);
        progressPollRef.current = null;
      }
    };
  }, [isProcessing]);

  const configLoaded = ingestionConfig !== null;
  const phase = derivePhase(configLoaded, aiConfigured, schemas);
  const phaseConf = PHASE_CONFIG[phase] || PHASE_CONFIG.has_data;

  // Seed welcome message when entering a phase for the first time
  const seededPhaseRef = useRef(null);
  useEffect(() => {
    if (phase === 'loading' || phase === 'needs_ai') return;
    if (seededPhaseRef.current === phase) return;
    seededPhaseRef.current = phase;
    if (phaseConf.welcome) {
      setMessages(prev => {
        if (prev.some(m => m.type === 'system' && m.content === phaseConf.welcome)) return prev;
        return [...prev, makeMsg('system', phaseConf.welcome)];
      });
    }
  }, [phase, phaseConf.welcome]);

  // Auto-scroll
  useEffect(() => {
    conversationEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  function makeMsg(type, content, data = null) {
    return {
      id: `msg-${Date.now()}-${Math.random().toString(36).slice(2)}`,
      type, content, data,
      timestamp: new Date().toISOString(),
    };
  }

  const addMessage = useCallback((type, content, data = null) => {
    setMessages(prev => [...prev, makeMsg(type, content, data)]);
  }, []);

  const processAgentResponse = useCallback((agentResponse) => {
    if (!agentResponse.success) {
      addMessage('system', `Error: ${agentResponse.error || 'Agent query failed'}`);
      return;
    }
    const result = agentResponse.data;
    if (result.session_id) setSessionId(result.session_id);

    // Render rich tool results inline and capture context for next turn
    if (result.tool_calls?.length > 0) {
      for (const tc of result.tool_calls) {
        if (tc.tool === 'scan_folder' && tc.result?.recommended_files) {
          addMessage('scan_result', 'Folder scan results', tc.result);
          // Save scan result so next request can attach it as context
          lastToolContextRef.current = { scan_result: tc.result, scan_params: tc.params };
        } else if (tc.tool === 'ingest_files' && tc.result?.results) {
          addMessage('ingest_result', 'Ingestion results', tc.result);
          lastToolContextRef.current = null; // clear after ingestion completes
        } else if (tc.tool === 'query' && Array.isArray(tc.result) && tc.result.length > 0) {
          addMessage('query_result', `Query: ${tc.params?.schema_name || 'data'}`, tc);
        }
      }
    }

    addMessage('system', result.answer);

    if (result.tool_calls) {
      const images = extractImagesFromToolCalls(result.tool_calls);
      if (images.length > 0) {
        addMessage('images', `${images.length} image(s)`, images);
      }
      if (result.tool_calls.some(tc => tc.tool === 'ingest_files')) {
        dispatch(fetchSchemas());
      }
    }
  }, [addMessage, dispatch]);

  const handleSubmit = useCallback(async (text) => {
    const userInput = (text || inputText).trim();
    if (!userInput || isProcessing) return;
    setInputText('');
    setIsProcessing(true);
    addMessage('user', userInput);

    try {
      const requestPayload = {
        query: userInput,
        session_id: sessionId,
        max_iterations: 15,
      };
      // Attach last tool context (e.g. scan results) so the LLM can reference exact data
      if (lastToolContextRef.current) {
        requestPayload.context = lastToolContextRef.current;
      }
      const agentResponse = await llmQueryClient.agentQuery(requestPayload);
      processAgentResponse(agentResponse);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      addMessage('system', `Error: ${msg}`);
    } finally {
      setIsProcessing(false);
    }
  }, [inputText, isProcessing, sessionId, addMessage, processAgentResponse]);

  const onFormSubmit = useCallback((e) => {
    e?.preventDefault();
    handleSubmit();
  }, [handleSubmit]);

  // Phase: Loading
  if (!configLoaded) {
    return (
      <div className="flex items-center justify-center h-[400px] text-secondary">
        <div className="text-center">
          <div className="w-5 h-5 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-3" />
          <p className="text-sm">Loading configuration...</p>
        </div>
      </div>
    );
  }

  // Phase: Needs AI Configuration
  if (!aiConfigured) {
    return (
      <div className="max-w-xl mx-auto py-8">
        <div className="text-center mb-6">
          <h2 className="text-lg font-medium text-primary mb-2">Configure AI Provider</h2>
          <p className="text-sm text-secondary">
            FoldDB needs an AI provider to ingest and search your data. Choose Anthropic (cloud) or Ollama (local).
          </p>
        </div>
        <div className="card p-6">
          {aiConfig.content}
          <div className="mt-4 flex justify-end">
            <button onClick={aiConfig.saveAiConfig} className="btn-primary">
              Save & Continue
            </button>
          </div>
        </div>
      </div>
    );
  }

  // Chat interface
  return (
    <div className="flex flex-col h-[600px]">
      {/* Status bar */}
      <div className="flex items-center justify-between px-6 py-2 border-b border-border text-xs text-tertiary">
        <span>{aiProvider} ({activeModel}) {phase === 'has_data' ? `\u00b7 ${schemas.length} schema(s)` : '\u00b7 no data yet'}</span>
        <button
          onClick={() => {
            setMessages([]);
            setSessionId(null);
            seededPhaseRef.current = null;
            lastToolContextRef.current = null;
          }}
          className="text-gruvbox-blue hover:underline bg-transparent border-none cursor-pointer text-xs"
        >
          New Conversation
        </button>
      </div>

      {/* Messages */}
      <div className="flex-1 overflow-y-auto p-6 space-y-3">
        {messages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-secondary">
            <p className="text-base mb-4">{phaseConf.heading}</p>
            <div className="flex flex-wrap gap-2 justify-center max-w-lg">
              {phaseConf.suggestions.map((s) => (
                <button
                  key={s.label}
                  onClick={() => s.text.endsWith(' ') ? setInputText(s.text) : handleSubmit(s.text)}
                  className="border border-border rounded-lg px-4 py-2 text-sm text-secondary hover:bg-surface-hover cursor-pointer bg-transparent"
                >
                  {s.label}
                </button>
              ))}
            </div>
          </div>
        ) : (
          messages.map((entry) => (
            <div key={entry.id} className="mb-2">
              {entry.type === 'user' && (
                <div className="flex justify-end">
                  <div className="max-w-[80%] px-4 py-3 bg-gruvbox-elevated border border-gruvbox-orange text-primary rounded-lg">
                    <p className="text-xs opacity-70 mb-1">You</p>
                    <p className="text-sm whitespace-pre-wrap">{entry.content}</p>
                  </div>
                </div>
              )}

              {entry.type === 'system' && (
                <div className="flex justify-start">
                  <div className="max-w-[80%] px-4 py-3 bg-surface-secondary border border-border rounded-lg">
                    <p className="text-xs text-tertiary mb-1">Assistant</p>
                    <p className="text-sm text-primary whitespace-pre-wrap">{entry.content}</p>
                  </div>
                </div>
              )}

              {entry.type === 'scan_result' && entry.data && (
                <ScanResultCard data={entry.data} />
              )}

              {entry.type === 'ingest_result' && entry.data && (
                <IngestResultCard data={entry.data} />
              )}

              {entry.type === 'query_result' && entry.data && (
                <QueryResultCard data={entry.data} />
              )}

              {entry.type === 'images' && Array.isArray(entry.data) && entry.data.length > 0 && (
                <div className="flex justify-start">
                  <div className="max-w-[80%] px-4 py-3 bg-surface-secondary border border-border rounded-lg">
                    <p className="text-xs text-tertiary mb-2">{entry.data.length} image{entry.data.length !== 1 ? 's' : ''}</p>
                    <div className="flex flex-wrap gap-2">
                      {entry.data.map((img) => (
                        <ImageThumbnail key={img.fileHash} fileHash={img.fileHash} sourceFile={img.sourceFile} />
                      ))}
                    </div>
                  </div>
                </div>
              )}
            </div>
          ))
        )}

        {isProcessing && (
          <div className="flex justify-start mb-3">
            <div className="px-4 py-3 bg-surface-secondary border border-border rounded-lg min-w-[200px]">
              {activeProgress ? (
                <div>
                  <p className="text-xs text-tertiary mb-1">{activeProgress.current_step || 'Processing'}</p>
                  <p className="text-sm text-primary mb-2">{activeProgress.status_message || 'Working...'}</p>
                  <div className="w-full bg-surface rounded-full h-2 overflow-hidden">
                    <div
                      className="h-full bg-gruvbox-green rounded-full transition-all duration-300"
                      style={{ width: `${activeProgress.progress_percentage || 0}%` }}
                    />
                  </div>
                  <p className="text-xs text-tertiary mt-1">{activeProgress.progress_percentage || 0}%</p>
                </div>
              ) : (
                <div className="flex items-center gap-1">
                  <span className="w-2 h-2 rounded-full bg-tertiary animate-bounce" style={{ animationDelay: '0ms' }} />
                  <span className="w-2 h-2 rounded-full bg-tertiary animate-bounce" style={{ animationDelay: '150ms' }} />
                  <span className="w-2 h-2 rounded-full bg-tertiary animate-bounce" style={{ animationDelay: '300ms' }} />
                </div>
              )}
            </div>
          </div>
        )}
        <div ref={conversationEndRef} />
      </div>

      {/* Single unified input */}
      <form onSubmit={onFormSubmit} className="px-6 py-4 border-t border-border bg-surface">
        <div className="flex gap-2">
          <input
            type="text"
            value={inputText}
            onChange={(e) => setInputText(e.target.value)}
            placeholder={phaseConf.placeholder}
            disabled={isProcessing}
            className="input flex-1"
            autoFocus
          />
          <button type="submit" disabled={!inputText.trim() || isProcessing} className="btn-primary btn-lg">
            {isProcessing ? 'Working...' : 'Send'}
          </button>
        </div>
      </form>
    </div>
  );
}

/** Render scan_folder results as a file browser */
function ScanResultCard({ data }) {
  const [expanded, setExpanded] = useState(false);
  const recommended = data.recommended_files || [];
  const skipped = data.skipped_files || [];
  const summary = data.summary || {};

  return (
    <div className="flex justify-start">
      <div className="max-w-[90%] bg-surface-secondary border border-border rounded-lg overflow-hidden">
        <div className="px-4 py-3 border-b border-border">
          <p className="text-xs text-tertiary mb-1">Scan Results</p>
          <div className="flex items-center gap-4 text-sm">
            <span className="text-gruvbox-green font-medium">{recommended.length} to ingest</span>
            <span className="text-tertiary">{skipped.length} skipped</span>
            {data.total_estimated_cost > 0 && (
              <span className="text-tertiary">~${data.total_estimated_cost.toFixed(4)}</span>
            )}
          </div>
          {Object.keys(summary).length > 0 && (
            <div className="flex flex-wrap gap-2 mt-2">
              {Object.entries(summary).map(([cat, count]) => (
                <span key={cat} className="text-xs px-2 py-0.5 bg-surface border border-border rounded">
                  {cat}: {count}
                </span>
              ))}
            </div>
          )}
        </div>
        <div className={`overflow-y-auto px-4 py-2 ${expanded ? 'max-h-80' : 'max-h-40'}`}>
          {recommended.map((f, i) => (
            <div key={i} className="flex items-center justify-between py-1 text-xs border-b border-border last:border-0">
              <span className="text-primary font-mono truncate flex-1 mr-2">{f.path}</span>
              <span className="text-gruvbox-green whitespace-nowrap mr-2">{f.category}</span>
              {f.file_size_bytes > 0 && (
                <span className="text-tertiary whitespace-nowrap">{formatBytes(f.file_size_bytes)}</span>
              )}
            </div>
          ))}
        </div>
        {recommended.length > 5 && (
          <div className="px-4 py-2 border-t border-border">
            <button
              onClick={() => setExpanded(!expanded)}
              className="text-xs text-gruvbox-blue hover:underline bg-transparent border-none cursor-pointer"
            >
              {expanded ? 'Show less' : `Show all ${recommended.length} files`}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** Render ingest_files results */
function IngestResultCard({ data }) {
  const [showDetails, setShowDetails] = useState(false);
  const results = data.results || [];

  return (
    <div className="flex justify-start">
      <div className="max-w-[90%] bg-surface-secondary border border-border rounded-lg overflow-hidden">
        <div className="px-4 py-3 border-b border-border">
          <p className="text-xs text-tertiary mb-1">Ingestion Results</p>
          <div className="flex items-center gap-4 text-sm">
            <span className="text-gruvbox-green font-medium">{data.succeeded} succeeded</span>
            {data.failed > 0 && <span className="text-gruvbox-red">{data.failed} failed</span>}
            <span className="text-tertiary">{data.total} total</span>
          </div>
        </div>
        {showDetails && (
          <div className="max-h-48 overflow-y-auto px-4 py-2">
            {results.map((r, i) => (
              <div key={i} className="flex items-center justify-between py-1 text-xs border-b border-border last:border-0">
                <span className="font-mono truncate flex-1 mr-2 text-primary">{r.file}</span>
                {r.success ? (
                  <span className="text-gruvbox-green whitespace-nowrap">{r.schema_used || 'OK'}</span>
                ) : (
                  <span className="text-gruvbox-red whitespace-nowrap">failed</span>
                )}
              </div>
            ))}
          </div>
        )}
        <div className="px-4 py-2 border-t border-border">
          <button
            onClick={() => setShowDetails(!showDetails)}
            className="text-xs text-gruvbox-blue hover:underline bg-transparent border-none cursor-pointer"
          >
            {showDetails ? 'Hide details' : 'Show file details'}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Render query results as a table */
function QueryResultCard({ data }) {
  const [showRaw, setShowRaw] = useState(false);
  const results = Array.isArray(data.result) ? data.result : [];
  const schemaName = data.params?.schema_name || 'Results';

  // Extract field names from first result
  const fields = results.length > 0
    ? Object.keys(results[0].fields || results[0] || {}).filter(f => f !== 'key')
    : [];

  return (
    <div className="flex justify-start">
      <div className="max-w-[95%] bg-surface-secondary border border-border rounded-lg overflow-hidden">
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <div>
            <p className="text-xs text-tertiary mb-0.5">Query: {schemaName}</p>
            <p className="text-xs text-secondary">{results.length} record(s)</p>
          </div>
          <button
            onClick={() => setShowRaw(!showRaw)}
            className="text-xs text-gruvbox-blue hover:underline bg-transparent border-none cursor-pointer"
          >
            {showRaw ? 'Table view' : 'Raw JSON'}
          </button>
        </div>
        <div className="max-h-64 overflow-auto">
          {showRaw ? (
            <pre className="text-xs font-mono p-3 overflow-x-auto">
              {JSON.stringify(results, null, 2)}
            </pre>
          ) : results.length > 0 && fields.length > 0 ? (
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-border">
                  {fields.slice(0, 6).map(f => (
                    <th key={f} className="px-3 py-2 text-left text-tertiary font-medium">{f}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {results.slice(0, 20).map((row, i) => {
                  const r = row.fields || row;
                  return (
                    <tr key={i} className="border-b border-border last:border-0">
                      {fields.slice(0, 6).map(f => (
                        <td key={f} className="px-3 py-2 text-primary truncate max-w-[200px]">
                          {typeof r[f] === 'object' ? JSON.stringify(r[f]) : String(r[f] ?? '')}
                        </td>
                      ))}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ) : (
            <p className="px-4 py-3 text-xs text-tertiary">No records</p>
          )}
        </div>
      </div>
    </div>
  );
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export default AgentTab;
