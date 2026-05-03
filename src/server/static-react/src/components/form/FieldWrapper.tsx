import type { ReactNode } from 'react';

interface FieldWrapperProps {
  label?: string;
  name?: string;
  required?: boolean;
  error?: string;
  helpText?: string;
  children: ReactNode;
  className?: string;
}

function FieldWrapper({
  label,
  name,
  required = false,
  error,
  helpText,
  children,
  className = ''
}: FieldWrapperProps) {
  const fieldId = name ? `field-${name}` : `field-${Math.random().toString(36).slice(2, 11)}`;
  const hasError = Boolean(error);

  return (
    <div className={`space-y-2 ${className}`}>
      <label
        htmlFor={fieldId}
        className="label"
      >
        {label}
        {required && (
          <span className="ml-1 text-tertiary" aria-label="required">
            *
          </span>
        )}
      </label>

      <div className="relative">
        {children}
      </div>

      {hasError && (
        <p
          className="text-sm text-gruvbox-red"
          role="alert"
          aria-live="polite"
        >
          {error}
        </p>
      )}

      {helpText && !hasError && (
        <p className="text-xs text-secondary">
          {helpText}
        </p>
      )}
    </div>
  );
}

export default FieldWrapper;
