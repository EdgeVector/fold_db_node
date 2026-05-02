import { useCallback, useState } from 'react';

type RangeMode = 'range' | 'key' | 'prefix';

interface RangeValue {
  start?: string;
  end?: string;
  key?: string;
  keyPrefix?: string;
}

const MODE_TO_FIELDS: Record<RangeMode, (keyof RangeValue)[]> = {
  range: ['start', 'end'],
  key: ['key'],
  prefix: ['keyPrefix'],
};

const DEFAULT_MODES: RangeMode[] = ['range', 'key', 'prefix'];

function determineInitialMode(value: RangeValue = {}): RangeMode {
  if (value.start || value.end) return 'range';
  if (value.key) return 'key';
  if (value.keyPrefix) return 'prefix';
  return 'range';
}

function clearFieldsOutsideMode(value: RangeValue, mode: RangeMode): RangeValue {
  const allowed = new Set<string>(MODE_TO_FIELDS[mode] ?? []);
  return Object.fromEntries(
    Object.entries(value).filter(([field]) => allowed.has(field)),
  ) as RangeValue;
}

interface UseRangeModeResult {
  state: { activeMode: RangeMode; value: RangeValue };
  actions: {
    changeMode: (newMode: RangeMode) => void;
    updateValue: (field: keyof RangeValue, newValue: string) => void;
    clearValue: () => void;
    setValue: (newValue: RangeValue) => void;
  };
  getAvailableModes: () => RangeMode[];
  isValidMode: (mode: string) => boolean;
}

export function useRangeMode(
  initialValue: RangeValue = {},
  onChange?: (next: RangeValue) => void,
  allowedModes: RangeMode[] = DEFAULT_MODES,
): UseRangeModeResult {
  const [activeMode, setActiveMode] = useState<RangeMode>(() =>
    determineInitialMode(initialValue),
  );
  const [value, setValue] = useState<RangeValue>(initialValue);

  const emit = useCallback(
    (nextValue: RangeValue) => {
      setValue(nextValue);
      onChange?.(nextValue);
    },
    [onChange],
  );

  const changeMode = useCallback(
    (newMode: RangeMode) => {
      if (!allowedModes.includes(newMode)) return;
      setActiveMode(newMode);
      emit({});
    },
    [allowedModes, emit],
  );

  const updateValue = useCallback(
    (field: keyof RangeValue, newValue: string) => {
      const baseMode: RangeMode =
        field === 'start' || field === 'end'
          ? 'range'
          : field === 'key'
            ? 'key'
            : 'prefix';
      const nextMode: RangeMode = MODE_TO_FIELDS[activeMode]?.includes(field)
        ? activeMode
        : baseMode;
      setActiveMode(nextMode);
      emit({ ...clearFieldsOutsideMode(value, nextMode), [field]: newValue });
    },
    [activeMode, emit, value],
  );

  const clearValue = useCallback(() => emit({}), [emit]);

  const setEntireValue = useCallback(
    (newValue: RangeValue) => {
      setActiveMode(determineInitialMode(newValue));
      emit(newValue);
    },
    [emit],
  );

  return {
    state: { activeMode, value },
    actions: {
      changeMode,
      updateValue,
      clearValue,
      setValue: setEntireValue,
    },
    getAvailableModes: useCallback(() => allowedModes, [allowedModes]),
    isValidMode: useCallback(
      (mode: string) => allowedModes.includes(mode as RangeMode),
      [allowedModes],
    ),
  };
}

export default useRangeMode;
