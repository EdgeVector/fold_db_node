import { useState } from 'react'
import ConfigureAiStep from './ConfigureAiStep'
import CloudBackupStep from './CloudBackupStep'

const STEPS = [
  { id: 'ai', label: 'AI Setup' },
  { id: 'cloud-backup', label: 'Cloud Backup' },
]

export default function OnboardingWizard({ onComplete }) {
  const [currentStep, setCurrentStep] = useState(0)

  const handleNext = () => {
    if (currentStep < STEPS.length - 1) {
      setCurrentStep(currentStep + 1)
    } else {
      localStorage.setItem('folddb_onboarding_complete', '1')
      onComplete()
    }
  }

  const handleSkip = () => {
    handleNext()
  }

  const stepId = STEPS[currentStep].id

  return (
    <div className="h-screen flex items-center justify-center bg-surface-secondary">
      <div className="w-full max-w-lg p-6 bg-surface border border-border">
        {/* Progress indicator */}
        <div className="flex items-center gap-2 mb-6">
          {STEPS.map((step, i) => (
            <div key={step.id} className="flex items-center gap-2 flex-1">
              <div className={`flex items-center justify-center w-6 h-6 text-xs font-bold border ${
                i < currentStep
                  ? 'bg-gruvbox-green text-surface border-gruvbox-green'
                  : i === currentStep
                    ? 'bg-surface text-gruvbox-blue border-gruvbox-blue'
                    : 'bg-surface text-tertiary border-border'
              }`}>
                {i < currentStep ? '\u2713' : i + 1}
              </div>
              <span className={`text-xs ${
                i === currentStep ? 'text-primary font-bold' : 'text-tertiary'
              }`}>
                {step.label}
              </span>
              {i < STEPS.length - 1 && (
                <div className={`flex-1 h-px ${
                  i < currentStep ? 'bg-gruvbox-green' : 'bg-border'
                }`} />
              )}
            </div>
          ))}
        </div>

        {/* Step content */}
        {stepId === 'ai' && (
          <ConfigureAiStep onNext={handleNext} onSkip={handleSkip} />
        )}
        {stepId === 'cloud-backup' && (
          <CloudBackupStep onNext={handleNext} onSkip={handleSkip} />
        )}
      </div>
    </div>
  )
}
