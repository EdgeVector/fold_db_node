/**
 * RangeField Helper Utilities
 * TASK-009: Additional Simplification - RangeField complexity reduction
 *
 * Extracted from RangeField.jsx to reduce component complexity and improve reusability.
 * These utilities handle help text generation, mode configuration, and range field styling.
 */

export type RangeFieldMode = 'all' | 'range' | 'key' | 'prefix';
export type RangeFieldActiveMode = 'range' | 'key' | 'prefix';

export interface RangeModeConfig {
  showModeSelector: boolean;
  availableModes: RangeFieldActiveMode[];
  defaultMode: RangeFieldActiveMode;
}

export interface VisibleFields {
  showRange: boolean;
  showKey: boolean;
  showPrefix: boolean;
}

export interface RangeConfigInput {
  mode?: RangeFieldMode;
  rangeKeyName?: string;
  required?: boolean;
  disabled?: boolean;
  className?: string;
}

export interface RangeConfig {
  mode: RangeFieldMode;
  rangeKeyName: string;
  required: boolean;
  disabled: boolean;
  className: string;
}

/** Generates help text for range field based on mode and range key name. */
export function generateRangeHelpText(
  mode: RangeFieldMode = 'all',
  rangeKeyName = 'key',
  customHelpText = '',
): string | null {
  if (customHelpText) return customHelpText;

  if (mode !== 'all') return null;

  return `Filter by ${rangeKeyName}: use exact ${rangeKeyName}, ${rangeKeyName} range, or ${rangeKeyName} prefix. Leave empty to return all records.`;
}

/** Gets the mode configuration for range field. */
export function getRangeModeConfig(mode: RangeFieldMode = 'all'): RangeModeConfig {
  const configs: Record<RangeFieldMode, RangeModeConfig> = {
    all: {
      showModeSelector: true,
      availableModes: ['range', 'key', 'prefix'],
      defaultMode: 'range',
    },
    range: {
      showModeSelector: false,
      availableModes: ['range'],
      defaultMode: 'range',
    },
    key: {
      showModeSelector: false,
      availableModes: ['key'],
      defaultMode: 'key',
    },
    prefix: {
      showModeSelector: false,
      availableModes: ['prefix'],
      defaultMode: 'prefix',
    },
  };

  return configs[mode] || configs.all;
}

/** Gets mode button styling. */
export function getModeButtonStyles(isActive = false): string {
  const baseStyles = 'px-3 py-1 text-xs transition-colors duration-200';

  if (isActive) {
    return `${baseStyles} bg-gruvbox-orange text-surface`;
  }

  return `${baseStyles} bg-gruvbox-elevated text-secondary hover:bg-gruvbox-hover`;
}

/** Gets the mode display labels. */
export function getModeLabels(): Record<RangeFieldActiveMode, string> {
  return {
    range: 'Key Range',
    key: 'Exact Key',
    prefix: 'Key Prefix',
  };
}

/** Determines which fields should be visible based on mode and configuration. */
export function getVisibleFields(mode: RangeFieldMode, activeMode: RangeFieldActiveMode): VisibleFields {
  if (mode === 'all') {
    return {
      showRange: activeMode === 'range',
      showKey: activeMode === 'key',
      showPrefix: activeMode === 'prefix',
    };
  }

  return {
    showRange: mode === 'range',
    showKey: mode === 'key',
    showPrefix: mode === 'prefix',
  };
}

/** Validates range field configuration. */
export function validateRangeConfig(config: RangeConfigInput = {}): RangeConfig {
  const {
    mode = 'all',
    rangeKeyName = 'key',
    required = false,
    disabled = false,
    className = '',
  } = config;

  const validModes: RangeFieldMode[] = ['all', 'range', 'key', 'prefix'];
  const validatedMode: RangeFieldMode = validModes.includes(mode) ? mode : 'all';

  return {
    mode: validatedMode,
    rangeKeyName: String(rangeKeyName),
    required: Boolean(required),
    disabled: Boolean(disabled),
    className: String(className),
  };
}

/** Gets the range field container styling. */
export function getRangeFieldContainerStyles(): string {
  return 'bg-gruvbox-elevated border border-border p-4 space-y-4';
}

/** Gets the range key display styles. */
export function getRangeKeyDisplayStyles(): string {
  return 'text-sm font-medium text-primary';
}

/** Gets the mode selector container styles. */
export function getModeSelectorStyles(): string {
  return 'flex space-x-4 mb-4';
}

/** Gets the input grid styles. */
export function getInputGridStyles(): string {
  return 'grid grid-cols-1 md:grid-cols-3 gap-4';
}
