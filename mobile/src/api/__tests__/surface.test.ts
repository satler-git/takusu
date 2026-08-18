import {
  decodeSurfaceCommandResponse,
  decodeSurfaceEvent,
  decodeSurfaceSnapshot,
} from '../agentTypes';

describe('surface state decoding', () => {
  it('decodes a snapshot and both stream event kinds', () => {
    const snapshot = decodeSurfaceSnapshot({
      version: 1,
      scope: 'device',
      state: 'thinking',
      revision: 3,
      operation_id: 7,
      error: null,
    });
    expect(snapshot).toEqual({
      scope: 'device',
      state: 'thinking',
      revision: 3,
      operation_id: 7,
    });

    expect(
      decodeSurfaceEvent({
        type: 'snapshot',
        scope: 'device',
        state: 'idle',
        revision: 0,
      }),
    ).toEqual({
      type: 'snapshot',
      scope: 'device',
      state: 'idle',
      revision: 0,
    });
    expect(
      decodeSurfaceEvent({
        type: 'state_changed',
        scope: 'device',
        state: 'speaking',
        revision: 4,
      })?.type,
    ).toBe('state_changed');
  });

  it('validates command responses', () => {
    const response = decodeSurfaceCommandResponse({
      command: 'stop-tts',
      accepted: true,
      reason: null,
      snapshot: {
        scope: 'device',
        state: 'idle',
        revision: 5,
        operation_id: 7,
      },
    });
    expect(response.command).toBe('stop-tts');
    expect(response.accepted).toBe(true);
    expect(response.snapshot.operation_id).toBe(7);

    expect(() =>
      decodeSurfaceCommandResponse({
        command: 'future-command',
        accepted: true,
        snapshot: {
          scope: 'device',
          state: 'idle',
          revision: 5,
        },
      }),
    ).toThrow('Invalid surface command response');
  });

  it('rejects malformed snapshots and unknown stream events', () => {
    expect(() =>
      decodeSurfaceSnapshot({
        scope: 'device',
        state: 'future_state',
        revision: 1,
      }),
    ).toThrow('Invalid surface snapshot');
    expect(
      decodeSurfaceEvent({
        type: 'future_event',
        scope: 'device',
        state: 'idle',
        revision: 1,
      }),
    ).toBeUndefined();
    expect(
      decodeSurfaceEvent({
        type: 'state_changed',
        scope: 'device',
        state: 'idle',
        revision: -1,
      }),
    ).toBeUndefined();
  });
});
