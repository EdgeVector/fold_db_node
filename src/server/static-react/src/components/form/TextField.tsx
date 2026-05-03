import { useState, useEffect, useCallback, useRef, type ChangeEvent } from 'react';
import FieldWrapper from './FieldWrapper';
import { FORM_FIELD_DEBOUNCE_MS } from '../../constants/ui';
import {
  generateFieldId,
  hasFieldError,
  generateInputStyles,
  generateAriaAttributes,
  getLoadingSpinnerClasses
} from '../../utils/formHelpers';

interface TextFieldProps {
  name: string;
  label?: string;
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
  disabled?: boolean;
  error?: string;
  placeholder?: string;
  helpText?: string;
  type?: 'text' | 'number' | 'email' | 'password';
  debounced?: boolean;
  debounceMs?: number;
  className?: string;
}

function TextField({
  name,
  label,
  value,
  onChange,
  required = false,
  disabled = false,
  error,
  placeholder,
  helpText,
  type = 'text',
  debounced = false,
  debounceMs = FORM_FIELD_DEBOUNCE_MS,
  className = ''
}: TextFieldProps) {
  const [internalValue, setInternalValue] = useState(value);
  const [isDebouncing, setIsDebouncing] = useState(false);

  useEffect(() => {
    setInternalValue(value);
  }, [value]);

  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rafRef = useRef<number | null>(null);
  const onChangeRef = useRef(onChange);

  useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
      if (rafRef.current && typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function') {
        window.cancelAnimationFrame(rafRef.current);
      }
    };
  }, []);

  const debouncedOnChange = useCallback((newValue: string) => {
    setIsDebouncing(true);
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
    if (rafRef.current && typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function') {
      window.cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    const schedule = () => {
      timeoutRef.current = setTimeout(() => {
        onChangeRef.current(newValue);
        setIsDebouncing(false);
      }, debounceMs);
    };
    if (typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function') {
      rafRef.current = window.requestAnimationFrame(schedule);
    } else {
      setTimeout(schedule, 0);
    }
  }, [debounceMs]);

  const handleChange = (e: ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInternalValue(newValue);

    if (debounced) {
      debouncedOnChange(newValue);
    } else {
      onChange(newValue);
    }
  };

  const fieldId = generateFieldId(name);
  const hasError = hasFieldError(error);
  const inputStyles = generateInputStyles({ hasError, disabled });
  const ariaAttributes = generateAriaAttributes({
    fieldId,
    hasError,
    hasHelp: Boolean(helpText)
  });

  return (
    <FieldWrapper
      label={label}
      name={name}
      required={required}
      error={error}
      helpText={helpText}
      className={className}
    >
      <div className="relative">
        <input
          id={fieldId}
          name={name}
          type={type}
          value={internalValue}
          onChange={handleChange}
          placeholder={placeholder}
          required={required}
          disabled={disabled}
          className={inputStyles}
          {...ariaAttributes}
        />

        {debounced && isDebouncing && (
          <div className="absolute right-2 top-1/2 transform -translate-y-1/2">
            <div
              className={getLoadingSpinnerClasses({ size: 'md', color: 'primary' })}
              role="status"
              aria-label="Processing input"
            ></div>
          </div>
        )}
      </div>
    </FieldWrapper>
  );
}

export default TextField;
