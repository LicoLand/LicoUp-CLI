// Failure classes are stable privacy-safe codes, the same vocabulary the
// admission returns from `client_state_migration.rs`. A code is the only thing
// that ever crosses the reporting boundary: no message, path or stored value.
export class MigrationStateError extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}
