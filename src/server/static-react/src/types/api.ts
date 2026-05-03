export interface ApiResponse<T = unknown> {
  success: boolean;
  message?: string;
  error?: string;
  data?: T;
}

/**
 * Payload shape that tabs pass to the shared onResult callback. Either an
 * `{ error }` (failure path) or `{ success: true, data }` (success path) —
 * the `success` flag and `error` field are independently optional because
 * tabs were originally untyped JS that mixed both styles.
 */
export interface OperationResultPayload<T = unknown> {
  success?: boolean;
  data?: T;
  error?: string;
}

export interface VerificationResult {
  is_valid: boolean;
  timestamp_valid: boolean;
  owner_id?: string;
  permissions?: string[];
  error?: string;
}

export interface VerificationResponse {
  verification_result: VerificationResult;
}
