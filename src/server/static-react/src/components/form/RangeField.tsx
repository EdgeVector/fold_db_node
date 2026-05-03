import FieldWrapper from './FieldWrapper';
import TextField from './TextField';
import { useRangeMode } from '../../hooks/useRangeMode';
import {
  generateRangeHelpText,
  getRangeModeConfig,
  getModeButtonStyles,
  getModeLabels,
  getVisibleFields,
  validateRangeConfig,
  getRangeFieldContainerStyles,
  getRangeKeyDisplayStyles,
  getModeSelectorStyles,
  getInputGridStyles,
  type RangeConfigInput,
} from '../../utils/rangeFieldHelpers';

interface RangeValue {
  start?: string;
  end?: string;
  key?: string;
  keyPrefix?: string;
}

interface RangeFieldProps {
  name: string;
  label?: string;
  value?: RangeValue;
  onChange: (value: RangeValue) => void;
  error?: string;
  helpText?: string;
  config?: RangeConfigInput;
  className?: string;
}

function RangeField({
  name,
  label,
  value = {},
  onChange,
  error,
  helpText,
  config = {},
  className = ''
}: RangeFieldProps) {
  const validatedConfig = validateRangeConfig(config);
  const { mode, rangeKeyName, required, disabled } = validatedConfig;

  const modeConfig = getRangeModeConfig(mode);

  const rangeMode = useRangeMode(value, onChange, modeConfig.availableModes);
  const { state, actions } = rangeMode;

  const modeLabels = getModeLabels();
  const visibleFields = getVisibleFields(mode, state.activeMode);

  const generatedHelpText = generateRangeHelpText(mode, rangeKeyName, helpText);

  return (
    <FieldWrapper
      label={label ?? ''}
      name={name}
      required={required}
      error={error}
      helpText={generatedHelpText ?? undefined}
      className={className}
    >
      <div className={getRangeFieldContainerStyles()}>
        <div className="mb-3">
          <span className={getRangeKeyDisplayStyles()}>
            Range Key: {rangeKeyName}
          </span>
        </div>

        {modeConfig.showModeSelector && (
          <div className={getModeSelectorStyles()}>
            {modeConfig.availableModes.map((modeKey) => (
              <button
                key={modeKey}
                type="button"
                onClick={() => actions.changeMode(modeKey)}
                className={getModeButtonStyles(state.activeMode === modeKey)}
              >
                {modeLabels[modeKey]}
              </button>
            ))}
          </div>
        )}

        <div className={getInputGridStyles()}>
          {visibleFields.showRange && (
            <>
              <TextField
                name={`${name}-start`}
                label="Start Key"
                value={state.value.start || ''}
                onChange={(newValue) => actions.updateValue('start', newValue)}
                placeholder="Start key"
                disabled={disabled}
                className="col-span-1"
              />
              <TextField
                name={`${name}-end`}
                label="End Key"
                value={state.value.end || ''}
                onChange={(newValue) => actions.updateValue('end', newValue)}
                placeholder="End key"
                disabled={disabled}
                className="col-span-1"
              />
            </>
          )}

          {visibleFields.showKey && (
            <TextField
              name={`${name}-key`}
              label="Exact Key"
              value={state.value.key || ''}
              onChange={(newValue) => actions.updateValue('key', newValue)}
              placeholder={`Exact ${rangeKeyName} to match`}
              disabled={disabled}
              className="col-span-1"
            />
          )}

          {visibleFields.showPrefix && (
            <TextField
              name={`${name}-prefix`}
              label="Key Prefix"
              value={state.value.keyPrefix || ''}
              onChange={(newValue) => actions.updateValue('keyPrefix', newValue)}
              placeholder={`${rangeKeyName} prefix (e.g., 'user:')`}
              disabled={disabled}
              className="col-span-1"
            />
          )}
        </div>
      </div>
    </FieldWrapper>
  );
}

export default RangeField;
