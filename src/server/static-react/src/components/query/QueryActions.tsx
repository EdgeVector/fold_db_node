import { useState } from 'react';
import { useQueryState } from '../../hooks/useQueryState';
import { BUTTON_TEXT } from '../../constants/ui';

interface QueryActionsProps {
  onExecute?: (data: unknown) => Promise<void> | void;
  onExecuteQuery?: (data: unknown) => Promise<void> | void;
  onValidate?: (data: unknown) => Promise<void> | void;
  onSave?: (data: unknown) => Promise<void> | void;
  onSaveQuery?: (data: unknown) => Promise<void> | void;
  onClear?: () => void;
  onClearQuery?: () => void;
  disabled?: boolean;
  isExecuting?: boolean;
  isSaving?: boolean;
  showValidation?: boolean;
  showSave?: boolean;
  showClear?: boolean;
  className?: string;
  queryData?: unknown;
}

type ActionFn = ((data: unknown) => Promise<void> | void) | undefined;

function QueryActions({
  onExecute,
  onExecuteQuery,
  onValidate,
  onSave,
  onSaveQuery,
  onClear,
  onClearQuery,
  disabled = false,
  isExecuting = false,
  isSaving = false,
  showValidation = false,
  showSave = true,
  showClear = true,
  className = '',
  queryData
}: QueryActionsProps) {
  const [loadingAction, setLoadingAction] = useState<string | null>(null);
  const [_confirmAction, setConfirmAction] = useState<string | null>(null);
  const queryStateHook = useQueryState() as { clearState?: () => void };
  const clearState = queryStateHook.clearState;

  const handleAction = async (action: string, actionFn: ActionFn, data: unknown = null) => {
    if (!actionFn || disabled) return;

    try {
      setLoadingAction(action);
      await actionFn(data);
    } catch (error) {
      console.error(`${action} action failed:`, error);
    } finally {
      setLoadingAction(null);
      setConfirmAction(null);
    }
  };

  const handleExecute = () => {
    const executeHandler = onExecuteQuery || onExecute;
    handleAction('execute', executeHandler, queryData);
  };

  const handleValidate = () => {
    handleAction('validate', onValidate, queryData);
  };

  const handleSave = () => {
    const saveHandler = onSaveQuery || onSave;
    handleAction('save', saveHandler, queryData);
  };

  const handleClear = () => {
    const clearHandler = onClearQuery || onClear;
    if (clearHandler) {
      clearHandler();
    }
    if (clearState) {
      clearState();
    }
  };

  const buttonText = BUTTON_TEXT as Record<string, string>;

  return (
    <div className={`flex justify-end gap-3 ${className}`}>
      {showClear && (
        <button type="button" onClick={handleClear} disabled={disabled} className="btn-secondary">
          {buttonText.clearQuery || 'Clear Query'}
        </button>
      )}

      {showValidation && onValidate && (
        <button type="button" onClick={handleValidate} disabled={disabled} className="btn-secondary flex items-center gap-2">
          {loadingAction === 'validate' && <span className="spinner" />}
          {buttonText.validateQuery || 'Validate'}
        </button>
      )}

      {showSave && (onSave || onSaveQuery) && (
        <button type="button" onClick={handleSave} disabled={disabled || isSaving} className="btn-secondary flex items-center gap-2">
          {(loadingAction === 'save' || isSaving) && <span className="spinner" />}
          {buttonText.saveQuery || 'Save Query'}
        </button>
      )}

      <button type="button" onClick={handleExecute} disabled={disabled || isExecuting} className="btn-primary flex items-center gap-2">
        {(loadingAction === 'execute' || isExecuting) && <span className="spinner" />}
        {(loadingAction === 'execute' || isExecuting) ? 'Executing...' : (buttonText.executeQuery || '→ Execute Query')}
      </button>
    </div>
  );
}

export default QueryActions;
