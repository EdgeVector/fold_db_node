import type { ChangeEvent } from 'react';
import FieldWrapper from './FieldWrapper';
import { UI_STATES } from '../../constants/ui';
import { useSearchableSelect } from '../../hooks/useSearchableSelect';
import { generateFieldId } from '../../utils/formHelpers';
import {
  processSelectConfig,
  getSelectStyles,
  createAriaAttributes,
  groupOptions,
  type SelectOption,
  type SelectConfigInput
} from '../../utils/selectFieldHelpers';

type SelectOptionWithDisabled = SelectOption & { disabled?: boolean };

interface SelectFieldProps {
  name: string;
  label?: string;
  value: string;
  options?: SelectOptionWithDisabled[];
  onChange: (value: string) => void;
  error?: string;
  helpText?: string;
  config?: SelectConfigInput;
  className?: string;
}

function SelectField({
  name,
  label,
  value,
  options = [],
  onChange,
  error,
  helpText,
  config = {},
  className = '',
}: SelectFieldProps) {
  const processedConfig = processSelectConfig(config);
  const { searchable, placeholder, emptyMessage, required, disabled, loading } = processedConfig;

  const fieldId = generateFieldId(name);
  const hasError = Boolean(error);
  const hasOptions = options.length > 0;

  const searchableSelect = useSearchableSelect(options, onChange, true);

  const handleStandardChange = (e: ChangeEvent<HTMLSelectElement>) => {
    onChange(e.target.value);
  };

  if (loading) {
    return (
      <FieldWrapper label={label ?? ''} name={name} required={required} error={error} helpText={helpText} className={className}>
        <div className="select opacity-50 cursor-not-allowed flex items-center">
          <span className="spinner mr-2" />
          {UI_STATES.loading}
        </div>
      </FieldWrapper>
    );
  }

  if (!hasOptions) {
    return (
      <FieldWrapper label={label ?? ''} name={name} required={required} error={error} helpText={helpText} className={className}>
        <div className="select opacity-50 cursor-not-allowed">{emptyMessage}</div>
      </FieldWrapper>
    );
  }

  if (searchable) {
    const { state, actions } = searchableSelect;
    const handleSearchChange = (e: ChangeEvent<HTMLInputElement>) => {
      actions.setSearchTerm(e.target.value);
    };
    const handleOptionSelect = (option: SelectOptionWithDisabled) => {
      if (option.disabled) return;
      onChange(option.value);
      actions.closeDropdown();
    };

    return (
      <FieldWrapper label={label ?? ''} name={name} required={required} error={error} helpText={helpText} className={className}>
        <div className="relative">
          <input
            type="text"
            placeholder={`Search ${(label ?? '').toLowerCase()}...`}
            value={state.searchTerm}
            onChange={handleSearchChange}
            onFocus={() => actions.openDropdown()}
            className={`input ${hasError ? 'border-gruvbox-red' : ''}`}
          />
          {state.isOpen && state.filteredOptions.length > 0 && (
            <div className="absolute z-10 w-full mt-1 bg-surface border border-border shadow-lg max-h-60 overflow-auto">
              {Object.entries(state.groupedOptions).map(([groupName, gOpts]) => (
                <div key={groupName}>
                  {groupName !== 'default' && (
                    <div className="px-3 py-2 text-xs font-semibold text-tertiary bg-surface-secondary border-b">
                      {groupName}
                    </div>
                  )}
                  {(gOpts as SelectOptionWithDisabled[]).map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => handleOptionSelect(option)}
                      disabled={option.disabled}
                      className={`w-full text-left px-3 py-2 hover:bg-surface-secondary focus:bg-surface-secondary focus:outline-none ${
                        option.disabled ? 'text-tertiary cursor-not-allowed' : 'text-primary'
                      } ${value === option.value ? 'bg-gruvbox-orange text-surface' : ''}`}
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
        </div>
      </FieldWrapper>
    );
  }

  const groupedOpts = groupOptions(options);
  const selectStyles = getSelectStyles(null, hasError, disabled, loading);
  const ariaAttributes = createAriaAttributes(fieldId, hasError, helpText);

  return (
    <FieldWrapper label={label ?? ''} name={name} required={required} error={error} helpText={helpText} className={className}>
      <select
        id={fieldId}
        name={name}
        value={value}
        onChange={handleStandardChange}
        required={required}
        disabled={disabled}
        className={selectStyles}
        {...ariaAttributes}
      >
        <option value="" disabled={required}>
          {placeholder}
        </option>
        {Object.entries(groupedOpts).map(([groupName, gOpts]) => {
          const opts = gOpts as SelectOptionWithDisabled[];
          return groupName !== 'default' ? (
            <optgroup key={groupName} label={groupName}>
              {opts.map((option) => (
                <option key={option.value} value={option.value} disabled={option.disabled}>
                  {option.label}
                </option>
              ))}
            </optgroup>
          ) : (
            opts.map((option) => (
              <option key={option.value} value={option.value} disabled={option.disabled}>
                {option.label}
              </option>
            ))
          );
        })}
      </select>
    </FieldWrapper>
  );
}

export default SelectField;
