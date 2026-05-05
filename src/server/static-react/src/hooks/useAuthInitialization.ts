import { useEffect } from 'react'
import { useAppDispatch, useAppSelector } from '../store/hooks'
import { restoreSession, autoLogin, loadSystemPublicKey } from '../store/authSlice'
import {
  fetchIngestionConfig,
  fetchIngestionStatus,
  selectIngestionConfig,
  selectAiConfiguredFromStatus,
  selectAiProvider,
} from '../store/ingestionSlice'
import { BROWSER_CONFIG } from '../constants/config'

/**
 * Orchestrates authentication bootstrap:
 *   1. Restore session from localStorage, or auto-login with node identity
 *   2. Load system public key for display
 *   3. Fetch AI/ingestion config + status once authenticated
 *
 * Also exposes the setup-banner state, which mirrors `/api/ingestion/status`
 * directly — no localStorage dismissal flag.
 */
export function useAuthInitialization() {
  const dispatch = useAppDispatch()
  const {
    isAuthenticated,
    isLoading: isAuthLoading,
    error: authError,
  } = useAppSelector((state) => state.auth)

  // Restore session FIRST - this must run before other effects.
  // Always auto-login with node identity (public key is the sole identity source).
  useEffect(() => {
    const userId = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID)
    const userHash = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
    if (userId && userHash) {
      dispatch(restoreSession({ id: userId, hash: userHash }))
      return
    }
    dispatch(autoLogin())
  }, [dispatch])

  // Manual retry for the auth-bootstrap error screen. Re-runs autoLogin (the
  // /api/system/auto-identity probe). Cheap to call; safe to spam.
  const retryAuth = () => {
    dispatch(autoLogin())
  }

  // Load the system public key for display in Key Management tab
  useEffect(() => {
    dispatch(loadSystemPublicKey())
  }, [dispatch])

  // Fetch AI configuration + status on mount (after auth). Status is the
  // banner's source of truth; config drives the settings UI.
  useEffect(() => {
    if (isAuthenticated) {
      dispatch(fetchIngestionConfig())
      dispatch(fetchIngestionStatus())
    }
  }, [dispatch, isAuthenticated])

  const ingestionConfig = useAppSelector(selectIngestionConfig)
  const aiConfiguredFromStatus = useAppSelector(selectAiConfiguredFromStatus)
  const aiProvider = useAppSelector(selectAiProvider)

  // Banner reflects backend readiness directly. Hidden until the first
  // status response lands (null) so we don't flash "configure AI" while
  // /api/ingestion/status is still in flight; reappears automatically if
  // the user later un-configures (clears the API key, switches to an
  // un-installed Ollama, etc.) — no manual dismissal layer.
  const aiConfigured = aiConfiguredFromStatus === true
  const showSetupBanner =
    isAuthenticated && aiConfiguredFromStatus === false

  return {
    isAuthenticated,
    isAuthLoading,
    authError,
    retryAuth,
    ingestionConfig,
    aiConfigured,
    aiProvider,
    showSetupBanner,
  }
}
