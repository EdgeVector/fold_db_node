/**
 * SelectField Helper Utilities
 * TASK-009: Additional Simplification - SelectField complexity reduction
 *
 * Extracted from SelectField.jsx to reduce component complexity and improve reusability.
 * These utilities handle option grouping, filtering, and configuration processing.
 */

import { isValueEmpty } from "./rangeSchemaHelpers";

export interface SelectOption {
  value: string;
  label: string;
  group?: string;
}

export interface SelectConfigInput {
  placeholder?: string;
  emptyMessage?: string;
  searchable?: boolean;
  required?: boolean;
  disabled?: boolean;
  loading?: boolean;
  showConfirmation?: boolean;
  [key: string]: unknown;
}

export interface SelectConfig {
  placeholder: string;
  emptyMessage: string;
  searchable: boolean;
  required: boolean;
  disabled: boolean;
  loading: boolean;
  showConfirmation: boolean;
  [key: string]: unknown;
}

export interface SelectAriaAttributes {
  "aria-invalid": boolean;
  "aria-describedby"?: string;
}

/** Groups options by their `group` property. */
export function groupOptions(options: SelectOption[] = []): Record<string, SelectOption[]> {
  return options.reduce<Record<string, SelectOption[]>>((groups, option) => {
    const group = option.group || "default";
    if (!groups[group]) {
      groups[group] = [];
    }
    groups[group].push(option);
    return groups;
  }, {});
}

/** Filters options based on search term. */
export function filterOptions(options: SelectOption[] = [], searchTerm = ""): SelectOption[] {
  if (isValueEmpty(searchTerm)) return options;

  const lowerSearchTerm = searchTerm.toLowerCase();
  return options.filter(
    (option) =>
      option.label.toLowerCase().includes(lowerSearchTerm) ||
      option.value.toLowerCase().includes(lowerSearchTerm),
  );
}

/** Processes select field configuration and applies defaults. */
export function processSelectConfig(config: SelectConfigInput = {}): SelectConfig {
  return {
    placeholder: "Select an option...",
    emptyMessage: "No options available",
    searchable: false,
    required: false,
    disabled: false,
    loading: false,
    showConfirmation: false,
    ...config,
  };
}

/** Determines select field styling based on state. */
export function getSelectStyles(
  _styles: unknown,
  hasError = false,
  disabled = false,
  loading = false,
): string {
  let classes = "select";

  if (hasError) {
    classes += " border-gruvbox-red";
  }

  if (disabled || loading) {
    classes += " opacity-50 cursor-not-allowed";
  }

  return classes;
}

/** Creates ARIA attributes for select field accessibility. */
export function createAriaAttributes(
  fieldId: string,
  hasError = false,
  helpText = "",
): SelectAriaAttributes {
  const attributes: SelectAriaAttributes = {
    "aria-invalid": hasError,
  };

  if (hasError) {
    attributes["aria-describedby"] = `${fieldId}-error`;
  } else if (helpText) {
    attributes["aria-describedby"] = `${fieldId}-help`;
  }

  return attributes;
}
