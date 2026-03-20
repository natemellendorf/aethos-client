# Chat Attachments Roadmap

## Scope shipped now

### 1) Emoji support

- Chat compose and payload processing are UTF-8 safe.
- Emoji content is preserved end-to-end in outbound JSON payloads and inbound display parsing.

### 2) Attachment v1 (small files)

- App-level attachment object in chat payload:
  - `file_name`
  - `mime_type`
  - `size_bytes`
  - `content_b64`
- Max attachment size: `2 MB`.
- Validation is fail-fast on send.
- Attachments render in thread and can be downloaded from message bubbles.

## Phase 3 plan (large files)

Goal: support files larger than the v1 inline payload cap while preserving deterministic behavior.

### Proposed architecture

1. **File manifest message**
   - Send a lightweight chat payload referencing file transfer metadata only.
   - Include:
     - file id (sha256)
     - file name / mime
     - total size
     - chunk count
     - optional preview metadata

2. **Chunk objects**
   - Split file into fixed-size chunks (e.g. 128 KiB).
   - Each chunk stored as independent gossip object.
   - Chunk payload includes: file id, chunk index, total chunks, chunk bytes.

3. **Integrity and completion rules**
   - Verify each chunk hash (optional) and final full-file hash (required).
   - Mark transfer complete only when all chunks are present and final hash matches.

4. **Resumable download state**
   - Persist partially received chunks.
   - Continue fetching missing chunks on later sync encounters.

5. **UX states**
   - Sender: queued -> syncing -> sent
   - Receiver: available -> downloading -> complete / failed

6. **Limits and safeguards**
   - Max total file size policy.
   - Max chunks per file policy.
   - Background cleanup for abandoned partial files.

### Interop requirements

- Keep wire behavior deterministic and app-layer backward compatible.
- Treat unknown chunk/file message types as non-fatal.
- Never fabricate sender identity from transport metadata.

### Test plan for phase 3

- chunk ordering + reassembly correctness
- duplicate chunk idempotency
- resume after interruption
- hash mismatch rejection
- cross-platform interop (desktop <-> iOS)
