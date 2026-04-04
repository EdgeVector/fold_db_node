import { useState, useCallback } from 'react'
import { markOnboardingComplete } from '../../api/clients/systemClient'
import ConfigureAiStep from './ConfigureAiStep'
import AppleDataStep from './AppleDataStep'
import CloudBackupStep from './CloudBackupStep'
import DiscoveryStep from './DiscoveryStep'
import AllSetStep from './AllSetStep'

const STEPS = [
  { id: 'welcome', label: 'AI Setup', number: 1 },
  { id: 'apple-data', label: 'Apple Data', number: 2 },
  { id: 'cloud-backup', label: 'Cloud Backup', number: 3 },
  { id: 'discovery', label: 'Community', number: 4 },
  { id: 'all-set', label: 'All Set', number: 5 },
]

export const ONBOARDING_STORAGE_KEY = 'folddb_onboarding_complete'

function ProgressIndicator({ currentStep, steps }) {
  return (
    <div className="flex items-center justify-center gap-1 mb-6">
      {steps.map((step, i) => {
        const isCurrent = step.id === currentStep
        const isPast = steps.findIndex(s => s.id === currentStep) > i
        return (
          <div key={step.id} className="flex items-center gap-1">
            <div className="flex flex-col items-center">
              <div
                className={`w-7 h-7 flex items-center justify-center text-xs font-bold border transition-colors ${
                  isCurrent
                    ? 'border-gruvbox-yellow text-gruvbox-yellow bg-gruvbox-yellow/10'
                    : isPast
                      ? 'border-gruvbox-green text-gruvbox-green bg-gruvbox-green/10'
                      : 'border-border text-secondary'
                }`}
              >
                {isPast ? '\u2713' : step.number}
              </div>
              <span className={`text-[10px] mt-1 ${
                isCurrent ? 'text-gruvbox-yellow' : isPast ? 'text-gruvbox-green' : 'text-tertiary'
              }`}>
                {step.label}
              </span>
            </div>
            {i < steps.length - 1 && (
              <div className={`w-8 h-px mt-[-12px] ${isPast ? 'bg-gruvbox-green' : 'bg-border'}`} />
            )}
          </div>
        )
      })}
    </div>
  )
}

export default function OnboardingWizard({ onComplete }) {
  const [currentStep, setCurrentStep] = useState('welcome')
  const [completedSteps, setCompletedSteps] = useState(new Set())

  const markCompleted = useCallback((stepId) => {
    setCompletedSteps(prev => new Set([...prev, stepId]))
  }, [])

  const goToStep = useCallback((stepId) => {
    setCurrentStep(stepId)
  }, [])

  const handleFinish = useCallback(() => {
    localStorage.setItem(ONBOARDING_STORAGE_KEY, '1')
    // Persist onboarding completion on the backend so --empty-db can reset it
    markOnboardingComplete().catch(() => {
      // Best-effort — localStorage is the fallback
    })
    onComplete()
  }, [onComplete])

  const renderStep = () => {
    switch (currentStep) {
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
      case 'apple-data':
        return (
          <AppleDataStep
            onNext={() => {
              markCompleted('apple-data')
              goToStep('cloud-backup')
            }}
            onSkip={() => goToStep('cloud-backup')}
          />
        )
      case 'cloud-backup':
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

        <ProgressIndicator currentStep={currentStep} steps={STEPS} />

        <div className="card p-6">
          {renderStep()}
        </div>

        {currentStep !== 'all-set' && (
          <div className="text-center mt-4">
            <button
              onClick={handleFinish}
              className="text-xs text-tertiary hover:text-secondary bg-transparent border-none cursor-pointer transition-colors"
            >
              Skip setup entirely
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
