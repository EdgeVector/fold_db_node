/**
 * Form Utilities - Consolidated Form Helper Functions
 * TASK-008: Duplicate Code Detection and Elimination
 *
 * This module consolidates common form-related utilities that were
 * duplicated across multiple form components, providing a single
 * source of truth for form field generation, styling, and validation.
 */

/** Generates a consistent field ID for form fields */
export function generateFieldId(name: string): string {
  return `field-${name}`;
}

/** Checks if a field has an error state */
export function hasFieldError(error: string | null | undefined): boolean {
  return Boolean(error);
}

export interface InputStyleOptions {
  hasError: boolean;
  disabled: boolean;
  additionalClasses?: string;
}

/** Generates input styling classes based on field state */
export function generateInputStyles({
  hasError,
  disabled,
  additionalClasses = "",
}: InputStyleOptions): string {
  const baseStyles = "input";
  const stateStyles = hasError ? "border-gruvbox-red" : "";
  const disabledStyles = disabled ? "opacity-50 cursor-not-allowed" : "";

  return `${baseStyles} ${stateStyles} ${disabledStyles} ${additionalClasses}`.trim();
}

export interface AriaOptions {
  fieldId: string;
  hasError: boolean;
  hasHelp: boolean;
}

export interface AriaAttributes {
  "aria-invalid": boolean;
  "aria-describedby"?: string;
}

/** Generates ARIA attributes for form fields */
export function generateAriaAttributes({
  fieldId,
  hasError,
  hasHelp,
}: AriaOptions): AriaAttributes {
  const attributes: AriaAttributes = {
    "aria-invalid": hasError,
  };

  if (hasError) {
    attributes["aria-describedby"] = `${fieldId}-error`;
  } else if (hasHelp) {
    attributes["aria-describedby"] = `${fieldId}-help`;
  }

  return attributes;
}

export type SpinnerSize = "sm" | "md" | "lg";
export type SpinnerColor = "primary" | "gray" | "white";

export interface SpinnerOptions {
  size?: SpinnerSize;
  color?: SpinnerColor;
}

/** Common loading spinner CSS class string */
export function getLoadingSpinnerClasses({
  size = "sm",
  color = "primary",
}: SpinnerOptions = {}): string {
  const sizeClasses: Record<SpinnerSize, string> = {
    sm: "h-3 w-3",
    md: "h-4 w-4",
    lg: "h-5 w-5",
  };

  const colorClasses: Record<SpinnerColor, string> = {
    primary: "border-primary border-t-transparent",
    gray: "border-border border-t-transparent",
    white: "border-white border-t-transparent",
  };

  return `animate-spin ${sizeClasses[size]} border-2 ${colorClasses[color]} rounded-full`;
}
