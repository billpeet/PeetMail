/** An error the client is told about: an HTTP status, a stable code and a sentence. */
export class SyncError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export const badRequest = (message: string) => new SyncError(400, "bad_request", message);
