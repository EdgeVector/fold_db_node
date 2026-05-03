import { useState, useCallback, useMemo } from 'react'
import { markOnboardingComplete } from '../../api/clients/systemClient'
import IdentityStep from './IdentityStep'
import ConfigureAiStep from './ConfigureAiStep'
import AppleDataStep from './AppleDataStep'
import CloudBackupStep from './CloudBackupStep'
import DiscoveryStep from './DiscoveryStep'
import AllSetStep from './AllSetStep'

interface StepDef {
  id: StepId
  label: string
  number: number
}

type StepId = 'identity' | 'welcome' | 'apple-data' | 'cloud-backup' | 'discovery' | 'all-set'

const STEPS: StepDef[] = [
  { id: 'identity', label: 'Identity', number: 1 },
  { id: 'welcome', label: 'AI Setup', number: 2 },
  { id: 'apple-data', label: 'Apple Data', number: 3 },
  { id: 'cloud-backup', label: 'Cloud Backup', number: 4 },
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
  return (
    <div className="flex items-start justify-center gap-1 mb-6">
      {steps.map((step, i) => {
        const isCurrent = step.id === currentStep
        const isPast = steps.findIndex(s => s.id === currentStep) > i
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
  const [currentStep, setCurrentStep] = useState<StepId>('identity')
  const [completedSteps, setCompletedSteps] = useState<Set<StepId>>(new Set())
  const cloudActive = useMemo(() => isCloudAlreadyActive(), [])
  const visibleSteps = useMemo(
    () => STEPS.filter(s => !(s.id === 'cloud-backup' && cloudActive)),
    [cloudActive]
  )

  const markCompleted = useCallback((stepId: StepId) => {
    setCompletedSteps(prev => new Set([...prev, stepId]))
  }, [])

  const goToStep = useCallback((stepId: StepId) => {
    setCurrentStep(stepId)
  }, [])

  const handleFinish = useCallback(() => {
    localStorage.setItem(ONBOARDING_STORAGE_KEY, '1')
    markOnboardingComplete().catch(() => {
      // Best-effort — localStorage is the fallback
    })
    onComplete()
  }, [onComplete])

  const renderStep = () => {
    switch (currentStep) {
      case 'identity':
        return (
          <IdentityStep
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
            onNext={() => {
              markCompleted('welcome')
              goToStep('apple-data')
            }}
            onSkip={() => goToStep('apple-data')}
          />
        )
      case 'apple-data': {
        const afterApple = () => {
          if (cloudActive) {
            markCompleted('cloud-backup')
            goToStep('discovery')
          } else {
            goToStep('cloud-backup')
          }
        }
        return (
          <AppleDataStep
            onNext={() => {
              markCompleted('apple-data')
              afterApple()
            }}
            onSkip={afterApple}
          />
        )
      }
      case 'cloud-backup':
        if (cloudActive) {
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
                  goToStep('discovery')
                }}
              >
                Next
              </button>
            </div>
          )
        }
        return (
          <CloudBackupStep
            onNext={() => {
              markCompleted('cloud-backup')
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

        {currentStep !== 'all-set' && (
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
