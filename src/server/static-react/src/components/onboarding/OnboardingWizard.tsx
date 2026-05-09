import { useState, useCallback, useMemo } from 'react'
import { useAppDispatch, useAppSelector } from '../../store/hooks'
import { fetchIngestionConfig, fetchIngestionStatus } from '../../store/ingestionSlice'
import { autoLogin, NODE_NOT_PROVISIONED_ERROR } from '../../store/authSlice'
import { systemClient, type BootstrapRequest, type BootstrapResponse } from '../../api/clients/systemClient'
import { isApiError } from '../../api/core/errors'
import IdentityStep from './IdentityStep'
import ConfigureAiStep, { type AiStepFields } from './ConfigureAiStep'
import AppleDataStep from './AppleDataStep'
import CloudBackupStep, { type CloudStepFields } from './CloudBackupStep'
import DiscoveryStep from './DiscoveryStep'
import AllSetStep from './AllSetStep'
import RecoveryPhraseView from './RecoveryPhraseView'

interface StepDef {
  id: StepId
  label: string
  number: number
}

type StepId =
  | 'identity'
  | 'welcome'
  | 'cloud-backup'
  | 'recovery-phrase'
  | 'apple-data'
  | 'discovery'
  | 'all-set'

// User-visible progress indicator. The `recovery-phrase` view is a
// transient sub-screen of the cloud-backup step (after a successful
// bootstrap response carrying a fresh-mint phrase) and intentionally
// doesn't get its own dot.
const STEPS: StepDef[] = [
  { id: 'identity', label: 'Identity', number: 1 },
  { id: 'welcome', label: 'AI Setup', number: 2 },
  { id: 'cloud-backup', label: 'Cloud Backup', number: 3 },
  { id: 'apple-data', label: 'Apple Data', number: 4 },
  { id: 'discovery', label: 'Community', number: 5 },
  { id: 'all-set', label: 'All Set', number: 6 },
]

export const ONBOARDING_STORAGE_KEY = 'folddb_onboarding_complete'

export function isCloudAlreadyActive(): boolean {
  if (typeof window === 'undefined') return false
  return !!window.localStorage.getItem('exemem_api_key')
}

interface ProgressIndicatorProps {
  currentStep: StepId
  steps: StepDef[]
}

function ProgressIndicator({ currentStep, steps }: ProgressIndicatorProps) {
  // The recovery-phrase view shares the cloud-backup dot.
  const indicatorStep: StepId = currentStep === 'recovery-phrase' ? 'cloud-backup' : currentStep
  return (
    <div className="flex items-start justify-center gap-1 mb-6">
      {steps.map((step, i) => {
        const isCurrent = step.id === indicatorStep
        const isPast = steps.findIndex(s => s.id === indicatorStep) > i
        return (
          <div key={step.id} className="flex items-start gap-1">
            <div className="flex flex-col items-center">
              <div
                className={`w-7 h-7 flex items-center justify-center text-xs font-bold border transition-colors shrink-0 ${
                  isCurrent
                    ? 'border-gruvbox-yellow text-gruvbox-yellow bg-gruvbox-yellow/10'
                    : isPast
                      ? 'border-gruvbox-green text-gruvbox-green bg-gruvbox-green/10'
                      : 'border-border text-secondary'
                }`}
              >
                {isPast ? '✓' : step.number}
              </div>
              <span className={`block w-14 h-[26px] text-center text-[10px] leading-tight mt-1 ${
                isCurrent ? 'text-gruvbox-yellow' : isPast ? 'text-gruvbox-green' : 'text-tertiary'
              }`}>
                {step.label}
              </span>
            </div>
            {i < steps.length - 1 && (
              <div className={`w-4 h-px mt-[13px] shrink-0 ${isPast ? 'bg-gruvbox-green' : 'bg-border'}`} />
            )}
          </div>
        )
      })}
    </div>
  )
}

interface OnboardingWizardProps {
  onComplete: () => void
}

export default function OnboardingWizard({ onComplete }: OnboardingWizardProps) {
  const dispatch = useAppDispatch()
  // The wizard is the *only* path forward when autoLogin returned
  // 503 node_not_provisioned, so the "Skip setup entirely" escape hatch
  // would just dead-end the user. Hide it in that case.
  const provisioningRequired = useAppSelector(
    (s) => s.auth.error === NODE_NOT_PROVISIONED_ERROR,
  )
  const [currentStep, setCurrentStep] = useState<StepId>('identity')
  const [completedSteps, setCompletedSteps] = useState<Set<StepId>>(new Set())
  const cloudActive = useMemo(() => isCloudAlreadyActive(), [])
  const visibleSteps = useMemo(
    () => STEPS.filter(s => !(s.id === 'cloud-backup' && cloudActive)),
    [cloudActive]
  )

  // Aggregated form state. One bootstrap POST consumes all of this at the
  // end of the cloud-backup step.
  const [identityFields, setIdentityFields] = useState({
    displayName: '',
    contactHint: '',
    birthday: '',
  })
  const [aiFields, setAiFields] = useState<AiStepFields>({
    provider: 'Anthropic',
    anthropicApiKey: '',
    anthropicModel: 'claude-haiku-4-5-20251001',
    ollamaUrl: 'http://localhost:11434',
    ollamaModel: '',
  })
  const [cloudFields, setCloudFields] = useState<CloudStepFields>({
    inviteCode: '',
    recoveryPhrase: '',
  })

  const [submitting, setSubmitting] = useState(false)
  const [submitError, setSubmitError] = useState<string | null>(null)
  const [recoveryWords, setRecoveryWords] = useState<string[] | null>(null)

  const markCompleted = useCallback((stepId: StepId) => {
    setCompletedSteps(prev => new Set([...prev, stepId]))
  }, [])

  const goToStep = useCallback((stepId: StepId) => {
    setCurrentStep(stepId)
  }, [])

  const buildBootstrapRequest = useCallback(({
    enableCloud,
    useRecoveryPhrase,
  }: {
    enableCloud: boolean
    useRecoveryPhrase: boolean
  }): BootstrapRequest => {
    // Default the display name when the user skipped the identity step. The
    // backend requires a non-empty `name`, so silently filling in a
    // placeholder is preferable to blocking submission.
    const name = identityFields.displayName.trim() || 'FoldDB User'

    let aiProvider: BootstrapRequest['ai_provider']
    if (aiFields.provider === 'Anthropic') aiProvider = 'anthropic'
    else if (aiFields.provider === 'Ollama') aiProvider = 'ollama'
    else aiProvider = 'skip'

    return {
      name,
      email: identityFields.contactHint.trim() || null,
      birthday: identityFields.birthday.trim() || null,
      ai_provider: aiProvider,
      anthropic_api_key:
        aiFields.provider === 'Anthropic' ? aiFields.anthropicApiKey || null : null,
      ollama_url: aiFields.provider === 'Ollama' ? aiFields.ollamaUrl || null : null,
      ollama_model: aiFields.provider === 'Ollama' ? aiFields.ollamaModel || null : null,
      enable_cloud: enableCloud,
      invite_code: enableCloud ? cloudFields.inviteCode.trim() : null,
      recovery_phrase: useRecoveryPhrase
        ? cloudFields.recoveryPhrase.trim().toLowerCase()
        : null,
    }
  }, [identityFields, aiFields, cloudFields])

  const submitBootstrap = useCallback(async (opts: {
    enableCloud: boolean
    useRecoveryPhrase: boolean
  }) => {
    setSubmitting(true)
    setSubmitError(null)
    try {
      const req = buildBootstrapRequest(opts)
      const resp = await systemClient.bootstrap(req)
      const data: BootstrapResponse | undefined = resp.data
      if (!resp.success || !data) {
        throw new Error('Bootstrap response missing data')
      }

      if (data.cloud?.enabled) {
        // Persist `exemem_api_key` so the rest of the UI (and a future page
        // refresh) sees cloud as already active. The bootstrap handler
        // saved the canonical credentials in the OS keychain; localStorage
        // here is the UI-side flag the gating reads.
        localStorage.setItem('exemem_user_hash', data.cloud.exemem_user_hash)
      }

      // Refresh AI store now that the backend has saved the config.
      dispatch(fetchIngestionConfig())
      dispatch(fetchIngestionStatus())
      // Re-run autoLogin so a node that started with `node_not_provisioned`
      // becomes authenticated now that bootstrap has provisioned it. Without
      // this, the user would finish the wizard but App.tsx would still see
      // `auth.error === 'node_not_provisioned'` and re-render the wizard.
      dispatch(autoLogin())

      // The node is now provisioned with an identity card. Set the
      // localStorage marker so a reload after a partial wizard exit
      // (e.g. closing the tab after Cloud Backup but before "All Set")
      // doesn't re-fire the wizard.
      localStorage.setItem(ONBOARDING_STORAGE_KEY, '1')

      markCompleted('cloud-backup')

      if (data.recovery_phrase && data.recovery_phrase.length > 0) {
        setRecoveryWords(data.recovery_phrase)
        goToStep('recovery-phrase')
      } else {
        // Restore path — no fresh-mint phrase to display.
        goToStep('apple-data')
      }
    } catch (e) {
      const message = isApiError(e)
        ? (e.toUserMessage() || e.message)
        : e instanceof Error
          ? e.message
          : String(e)
      setSubmitError(message)
    } finally {
      setSubmitting(false)
    }
  }, [buildBootstrapRequest, dispatch, markCompleted, goToStep])

  const handleFinish = useCallback(() => {
    localStorage.setItem(ONBOARDING_STORAGE_KEY, '1')
    onComplete()
  }, [onComplete])

  const renderStep = () => {
    switch (currentStep) {
      case 'identity':
        return (
          <IdentityStep
            displayName={identityFields.displayName}
            contactHint={identityFields.contactHint}
            birthday={identityFields.birthday}
            onChange={(next) => setIdentityFields(prev => ({ ...prev, ...next }))}
            onNext={() => {
              markCompleted('identity')
              goToStep('welcome')
            }}
            onSkip={() => goToStep('welcome')}
          />
        )
      case 'welcome':
        return (
          <ConfigureAiStep
            fields={aiFields}
            onChange={(next) => setAiFields(prev => ({ ...prev, ...next }))}
            onNext={() => {
              markCompleted('welcome')
              goToStep(cloudActive ? 'apple-data' : 'cloud-backup')
            }}
            onSkip={() => goToStep(cloudActive ? 'apple-data' : 'cloud-backup')}
          />
        )
      case 'cloud-backup':
        if (cloudActive) {
          // Already-active gate stays here for parity with the previous
          // behavior, but the bootstrap runs only on a fresh node — so this
          // branch now only fires when the user re-enters the wizard with
          // an existing cloud session. Skip straight through.
          return (
            <div data-testid="cloud-already-active" className="text-center">
              <h2 className="text-lg font-bold text-primary mb-2">Cloud backup is already active</h2>
              <p className="text-xs text-secondary mb-4">
                You activated Exemem cloud during setup. Skipping this step.
              </p>
              <button
                className="btn btn-primary"
                onClick={() => {
                  markCompleted('cloud-backup')
                  goToStep('apple-data')
                }}
              >
                Next
              </button>
            </div>
          )
        }
        return (
          <CloudBackupStep
            fields={cloudFields}
            onChange={(next) => setCloudFields(prev => ({ ...prev, ...next }))}
            onEnableCloud={() => submitBootstrap({ enableCloud: true, useRecoveryPhrase: false })}
            onRestore={() => submitBootstrap({ enableCloud: true, useRecoveryPhrase: true })}
            onSkip={() => submitBootstrap({ enableCloud: false, useRecoveryPhrase: false })}
            submitting={submitting}
            error={submitError}
          />
        )
      case 'recovery-phrase':
        if (!recoveryWords) {
          // Defensive: should never render without words. Bounce to apple.
          return null
        }
        return (
          <RecoveryPhraseView
            words={recoveryWords}
            onContinue={() => goToStep('apple-data')}
          />
        )
      case 'apple-data':
        return (
          <AppleDataStep
            onNext={() => {
              markCompleted('apple-data')
              goToStep('discovery')
            }}
            onSkip={() => goToStep('discovery')}
          />
        )
      case 'discovery':
        return (
          <DiscoveryStep
            onNext={() => {
              markCompleted('discovery')
              goToStep('all-set')
            }}
            onSkip={() => goToStep('all-set')}
          />
        )
      case 'all-set':
        return (
          <AllSetStep
            onFinish={handleFinish}
            completedSteps={completedSteps}
          />
        )
      default:
        return null
    }
  }

  return (
    <div className="fixed inset-0 z-[1000] flex items-center justify-center bg-surface"
      style={{ fontFamily: "'IBM Plex Mono', monospace" }}
    >
      <div className="w-full max-w-lg mx-auto px-6">
        <div className="text-center mb-6">
          <h1 className="text-xl font-bold text-primary mb-1">FoldDB</h1>
          <p className="text-xs text-secondary">Your data, your rules</p>
        </div>

        <ProgressIndicator currentStep={currentStep} steps={visibleSteps} />

        <div className="card p-6">
          {renderStep()}
        </div>

        {currentStep !== 'all-set' && currentStep !== 'recovery-phrase' && !provisioningRequired && (
          <div className="text-center mt-4">
            <button
              onClick={handleFinish}
              className="text-xs text-tertiary hover:text-secondary bg-transparent border-none cursor-pointer transition-colors px-4 py-3 min-h-[44px]"
            >
              Skip setup entirely
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

export type { StepId }
