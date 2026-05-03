import {
  SCHEMA_BADGE_COLORS,
  RANGE_SCHEMA_CONFIG,
  HELP_TEXT
} from '../../constants/ui';

type SchemaState = 'approved' | 'available' | 'blocked' | 'pending';
type Size = 'sm' | 'md' | 'lg';

interface SchemaStatusBadgeProps {
  state: SchemaState | string;
  isRangeSchema?: boolean;
  size?: Size;
  className?: string;
  showTooltip?: boolean;
}

function SchemaStatusBadge({
  state,
  isRangeSchema = false,
  size = 'md',
  className = '',
  showTooltip = true
}: SchemaStatusBadgeProps) {
  const sizeClasses: Record<Size, string> = {
    sm: 'px-1.5 py-0.5 text-xs',
    md: 'px-2.5 py-0.5 text-xs',
    lg: 'px-3 py-1 text-sm'
  };

  const getBadgeColor = (): string => {
    return SCHEMA_BADGE_COLORS[state] || SCHEMA_BADGE_COLORS.available;
  };

  const getStateText = (): string => {
    const stateTexts: Record<string, string> = {
      approved: 'Approved',
      available: 'Available',
      blocked: 'Blocked',
      pending: 'Pending'
    };
    return stateTexts[state] || 'Unknown';
  };

  const getTooltipText = (): string => {
    if (!showTooltip) return '';
    const states = HELP_TEXT.schemaStates as Record<string, string>;
    return states[state] || 'Unknown schema state';
  };

  const badgeClasses = `
    inline-flex items-center rounded-full font-medium
    ${sizeClasses[size]}
    ${getBadgeColor()}
    ${className}
  `.trim();

  return (
    <div className="inline-flex items-center space-x-2">
      <span
        className={badgeClasses}
        title={getTooltipText()}
        aria-label={`Schema status: ${getStateText()}${isRangeSchema ? ', Range Schema' : ''}`}
      >
        {getStateText()}
      </span>

      {isRangeSchema && (
        <span
          className={`
            inline-flex items-center rounded-full font-medium
            ${sizeClasses[size]}
            ${RANGE_SCHEMA_CONFIG.badgeColor}
          `}
          title="This schema uses range-based keys for efficient querying"
          aria-label="Range Schema"
        >
          {RANGE_SCHEMA_CONFIG.label}
        </span>
      )}
    </div>
  );
}

export default SchemaStatusBadge;
