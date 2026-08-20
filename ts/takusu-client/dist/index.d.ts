interface components$1 {
    schemas: {
        /**
         * Format: double
         * @description A value in `[0.0, 1.0]` representing how easily a task can be abandoned.
         *
         *     Higher values mean the task is more likely to be dropped when the schedule
         *     does not fit. `new` silently clamps the input to `[0.0, 1.0]`; `NaN`
         *     becomes `0.5`.
         */
        Abandonability: number;
        AttachWorkSession: {
            task_id: string;
        };
        ClaimEventRequest: {
            device_id: string;
        };
        ClaimEventResponse: {
            claimed: boolean;
        };
        /**
         * @description Author of a task comment (`task_comments` table, WI-1).
         *
         *     `User` is assigned by the public comment endpoint, `Agent` by the
         *     agent-only endpoint, and `System` is created only server-side
         *     (migrations, hooks) — never accepted from any request.
         * @enum {string}
         */
        CommentAuthor: 'user' | 'agent' | 'system';
        /**
         * @description A single entry in a task's comment timeline (WI-1).
         *
         *     Comments are append-only: there is no edit operation, and `author` is
         *     server-assigned when the row is created. `seq` is a per-task monotonic
         *     sequence assigned by storage, so ordering is deterministic even when
         *     multiple rows share a `created_at` timestamp.
         */
        CommentRow: {
            author: components$1['schemas']['CommentAuthor'];
            content: string;
            created_at: components$1['schemas']['Timestamp'];
            id: string;
            /** Format: int64 */
            seq: number;
            task_id: string;
        };
        CompleteQuery: {
            /** Format: uint */
            limit?: number | null;
            q: string;
        };
        Completion: {
            /** @description Label shown in the completion UI. */
            label: string;
            /** @description Full query value after selecting this completion. */
            value: string;
        };
        ConvertWorkSession: {
            fixed?: boolean | null;
            status?: components$1['schemas']['TaskStatus'] | null;
            title?: string | null;
        };
        /**
         * @description A recorded coverage confirmation (WI-10).
         *
         *     Confirms that a local period was covered: the user (or an intake/capture
         *     flow) stated what happened during that interval.
         */
        CoverageConfirmationRow: {
            calendar_health: string;
            created_at: components$1['schemas']['Timestamp'];
            end_at: components$1['schemas']['Timestamp'];
            id: string;
            operation_id?: string | null;
            /** Format: int64 */
            schedule_revision: number;
            settled_at?: components$1['schemas']['Timestamp'] | null;
            source: string;
            start_at: components$1['schemas']['Timestamp'];
            timezone: string;
        };
        /** @description Coverage data assembled for one planner evaluation (WI-10). */
        CoverageEvaluation: {
            confirmations: components$1['schemas']['CoverageConfirmationRow'][];
            /** Format: int64 */
            schedule_revision: number;
            state: components$1['schemas']['CoverageState'];
            unsettled_intervals: components$1['schemas']['UnsettledIntervalRow'][];
        };
        /**
         * @description Coverage trust state consumed by the resident agent (WI-10).
         *
         *     Precedence is `bootstrap -> stale -> today-covered -> trusted`. A stale
         *     state triggers a settlement prompt; today-covered makes the current task
         *     authoritative; trusted is reached by a target-period procedure.
         */
        CoverageState: 'bootstrap' | 'today_covered' | 'trusted' | 'stale';
        /**
         * @description Request body for creating a task comment (WI-1).
         *
         *     Contains only `content`. `author` is deliberately absent: it is assigned by
         *     the server based on which endpoint is used (public `/tasks/:id/comments` →
         *     `user`, `/tasks/:id/comments/agent` → `agent`), so ordinary clients cannot
         *     impersonate the agent or system (invariant 2).
         */
        CreateComment: {
            content: string;
        };
        CreateHabit: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            description?: string | null;
            end_time: components$1['schemas']['TimeOfDay'];
            fixed?: boolean | null;
            parallelizable?: boolean | null;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
            /** @description Window mode: `'day'` or `'period'` (#window_mode). */
            window_mode?: components$1['schemas']['WindowMode'] | null;
        };
        /** @description Request body for `POST /api/habits/batch` (#1083). */
        CreateHabitBatch: {
            habits: components$1['schemas']['CreateHabitBatchItem'][];
        };
        /** @description A single habit inside a batch create request (#1083). */
        CreateHabitBatchItem: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            client_id?: string | null;
            description?: string | null;
            end_time: components$1['schemas']['TimeOfDay'];
            fixed?: boolean | null;
            parallelizable?: boolean | null;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
            /** @description Window mode: `'day'` or `'period'` (#window_mode). */
            window_mode?: components$1['schemas']['WindowMode'] | null;
        };
        /** @description A single result from a batch create request (#1083). */
        CreateHabitBatchResult: {
            abandonability: components$1['schemas']['Abandonability'];
            /** @default false */
            active: boolean;
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            client_id?: string | null;
            created_at: components$1['schemas']['Timestamp'];
            description?: string | null;
            /**
             * Format: int64
             * @default 0
             */
            display_id: number;
            end_time: components$1['schemas']['TimeOfDay'];
            /** @default false */
            fixed: boolean;
            id: string;
            /** @default false */
            parallelizable: boolean;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes: number;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
            updated_at: components$1['schemas']['Timestamp'];
            /**
             * @description Window mode for generated tasks (#window_mode).
             *     `'day'` (default) = occurrence day's start_time..end_time.
             *     `'period'` = occurrence start_time .. next occurrence's start_time.
             * @default day
             */
            window_mode: components$1['schemas']['WindowMode'];
        };
        CreateHabitScheduledSpan: {
            end_date: components$1['schemas']['Date'];
            reason?: string | null;
            start_date: components$1['schemas']['Date'];
        };
        CreateMemory: {
            content: string;
            key: string;
            kind: components$1['schemas']['MemoryKind'];
            subject_id?: string | null;
            subject_type?: components$1['schemas']['SubjectType'] | null;
            /** @default false */
            upsert: boolean;
        };
        CreateSkill: {
            body: string;
            built_in?: boolean | null;
            description: string;
            name: string;
            slug: string;
        };
        CreateTask: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            depends?: string[] | null;
            description?: string | null;
            end_at: components$1['schemas']['Timestamp'];
            fixed?: boolean | null;
            habit_id?: string | null;
            /** @description habit step link (#95). Set by sync_habit_tasks for step-generated tasks. */
            habit_step_id?: string | null;
            ical_uid?: string | null;
            /** @description WI-9: pre-split total quantity, kept for lineage. */
            original_quantity_total?: components$1['schemas']['Quantity'] | null;
            parallelizable?: boolean | null;
            /** @description WI-9: initial quantity already done (defaults to 0). */
            quantity_done?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: total quantity for a quantitative task. */
            quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: unit for the quantity. */
            quantity_unit?: string | null;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_at?: components$1['schemas']['Timestamp'] | null;
            title: string;
        };
        /** @description Request body for `POST /api/tasks/batch` (#1083). */
        CreateTaskBatch: {
            tasks: components$1['schemas']['CreateTaskBatchItem'][];
        };
        /**
         * @description A single task inside a batch create request (#1083).
         *     `client_id` is a caller-supplied temporary id that can be referenced by
         *     `depends` of other items in the same batch.
         */
        CreateTaskBatchItem: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            client_id?: string | null;
            depends?: string[] | null;
            description?: string | null;
            end_at: components$1['schemas']['Timestamp'];
            fixed?: boolean | null;
            habit_id?: string | null;
            /** @description habit step link (#95). Set by sync_habit_tasks for step-generated tasks. */
            habit_step_id?: string | null;
            ical_uid?: string | null;
            /** @description WI-9: pre-split total quantity, kept for lineage. */
            original_quantity_total?: components$1['schemas']['Quantity'] | null;
            parallelizable?: boolean | null;
            /** @description WI-9: initial quantity already done (defaults to 0). */
            quantity_done?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: total quantity for a quantitative task. */
            quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: unit for the quantity. */
            quantity_unit?: string | null;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_at?: components$1['schemas']['Timestamp'] | null;
            title: string;
        };
        /**
         * @description A single result from a batch create request (#1083).
         *     The caller can correlate results with input items by `client_id` and by
         *     position.
         */
        CreateTaskBatchResult: {
            abandonability: components$1['schemas']['Abandonability'];
            /**
             * Format: int64
             * @description Total active work minutes from work_sessions (NULL when no work has been done).
             */
            actual_minutes?: number | null;
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            client_id?: string | null;
            /** @description WI-9: wall-clock completion time, set by `complete`. */
            completed_at?: components$1['schemas']['Timestamp'] | null;
            created_at: components$1['schemas']['Timestamp'];
            /** @default [] */
            depends: components$1['schemas']['JsonString'];
            description?: string | null;
            /**
             * Format: int64
             * @default 0
             */
            display_id: number;
            end_at: components$1['schemas']['Timestamp'];
            /** @default false */
            fixed: boolean;
            habit_id?: string | null;
            /**
             * @description The habit step that generated this task, if any (#95). NULL for simple
             *     (step-less) habits and manually created tasks.
             */
            habit_step_id?: string | null;
            ical_uid?: string | null;
            id: string;
            /** @description WI-9: pre-split total quantity, kept for lineage. */
            original_quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @default false */
            parallelizable: boolean;
            /**
             * @description WI-9: quantity already done. Defaults to 0.
             * @default 0
             */
            quantity_done: components$1['schemas']['Quantity'];
            /** @description WI-9: total quantity for a quantitative task (e.g. 30 題). */
            quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: unit for the quantity (e.g. "題"). */
            quantity_unit?: string | null;
            /** Format: int64 */
            sigma_minutes: number;
            /** @description WI-9: for a remainder task, the id of the task it was split from. */
            split_from_task_id?: string | null;
            start_at?: components$1['schemas']['Timestamp'] | null;
            status: components$1['schemas']['TaskStatus'];
            title: string;
            updated_at: components$1['schemas']['Timestamp'];
            /** @default false */
            user_edited: boolean;
        };
        CreateTokenRequest: {
            label?: string | null;
        };
        /**
         * @description A calendar date in `YYYY-MM-DD` format.
         *
         *     Serialized as a `"YYYY-MM-DD"` string for JSON and stored as `TEXT` in
         *     SQLite. Wraps [`jiff::civil::Date`].
         */
        Date: string;
        DeleteAllGcalFailure: {
            error: string;
            task_id: string;
        };
        /** @description Result of explicitly deleting every mapped Google Calendar event. */
        DeleteAllGcalResult: {
            /** Format: uint */
            deleted: number;
            failed: components$1['schemas']['DeleteAllGcalFailure'][];
        };
        DeleteMemoryParams: {
            /** Format: int64 */
            observed_revision: number;
        };
        /**
         * @description Response for `GET /api/tasks/dependency-analysis` and the habit step
         *     variant (#355).
         */
        DependencyAnalysisResponse: {
            redundant: components$1['schemas']['RedundantDependency'][];
        };
        /** @description A node on a dependency witness path (task or habit step) (#355). */
        DependencyNode: {
            id: string;
            title: string;
        };
        /** @enum {string} */
        EstimatorBand: 'usual' | 'attention' | 'replan';
        EstimatorResult: {
            band: components$1['schemas']['EstimatorBand'];
            next_crossing_time?: components$1['schemas']['Timestamp'] | null;
            observation_id: string;
            /** Format: double */
            prior_shift_z?: number | null;
            /** Format: int64 */
            revision: number;
            /** Format: double */
            survival_probability: number;
        };
        EvaluateEventsRequest: {
            /** @default  */
            device_id: string;
        };
        /** @description Estimator distribution snapshot for a single task. */
        EvaluationEstimator: {
            /** Format: double */
            mean_minutes: number;
            /** Format: int64 */
            revision: number;
            /** Format: double */
            sigma_minutes: number;
        };
        /**
         * @description Raw inputs for one consistent planner-event evaluation.
         *
         *     The storage backend collects these in a single atomic read (or as close to
         *     atomic as the backend supports) so the pure evaluator receives a coherent
         *     snapshot. The caller still supplies `now`, gap classification, and coverage.
         */
        EvaluationInputs: {
            /**
             * @description Coverage trust state for the current evaluation (WI-10).
             * @default {
             *       "confirmations": [],
             *       "schedule_revision": 0,
             *       "state": "bootstrap",
             *       "unsettled_intervals": []
             *     }
             */
            coverage: components$1['schemas']['CoverageEvaluation'];
            ledger: components$1['schemas']['EventLedgerRow'][];
            progress: components$1['schemas']['EvaluationTaskProgress'][];
            schedule: components$1['schemas']['ScheduleEntry'][];
            /** Format: int64 */
            schedule_revision: number;
            tasks: components$1['schemas']['TaskRow'][];
        };
        /**
         * @description Per-task progress for evaluation. Only in-progress tasks are included; the
         *     estimator is pre-computed by the storage layer so callers do not have to
         *     re-derive the fallback distribution.
         */
        EvaluationTaskProgress: {
            estimator?: components$1['schemas']['EvaluationEstimator'] | null;
            task_id: string;
            /** Format: int64 */
            total_active_minutes: number;
        };
        /**
         * @description Delivery state persisted by the resident event ledger (WI-9).
         * @enum {string}
         */
        EventDeliveryState: 'pending_delivery' | 'delivered' | 'deferred_quiet_hours' | 'acknowledged' | 'ignored' | 'resolved';
        EventEvaluationResponse: {
            due_events: unknown[];
            next_eval_at?: string | null;
        };
        /** @description Values written when an evaluator commits a newly discovered event. */
        EventLedgerInsert: {
            /** Format: int64 */
            distribution_revision?: number | null;
            id: string;
            kind: string;
            observation_kind: string;
            presentation: string;
            /** Format: int64 */
            schedule_revision: number;
            task_id?: string | null;
            urgency: string;
        };
        /**
         * @description Storage representation of an immutable planner event.
         *
         *     Presentation and action templates remain JSON strings at this boundary so
         *     `takusu-contracts` does not depend on `takusu-agent`.
         */
        EventLedgerRow: {
            created_at: components$1['schemas']['Timestamp'];
            delivered_at?: components$1['schemas']['Timestamp'] | null;
            delivery_state: components$1['schemas']['EventDeliveryState'];
            /** Format: int64 */
            distribution_revision?: number | null;
            id: string;
            kind: string;
            observation_kind: string;
            presentation: string;
            /** Format: int64 */
            schedule_revision: number;
            task_id?: string | null;
            urgency: string;
        };
        EventListQuery: {
            device_id?: string | null;
        };
        /** @description Request body for `POST /api/schedule/generate`. */
        GenerateSchedule: {
            /** @default recommended */
            sleep: components$1['schemas']['SleepInput'];
            task_ids?: string[] | null;
        };
        GoogleCalEventRow: {
            google_event_id: string;
            task_id: string;
            updated_at: components$1['schemas']['Timestamp'];
        };
        GoogleCalSettingsOutput: {
            calendar_id: string;
            client_id: string;
            /** Format: int64 */
            color_id?: number | null;
            enabled: boolean;
            has_client_secret: boolean;
            has_refresh_token: boolean;
            /** Format: int64 */
            reminder_minutes?: number | null;
            transparency?: string | null;
            visibility?: string | null;
        };
        /**
         * @description Habit detail response: the habit row plus its steps (#95). Used by
         *     `GET /api/habits/:id` so clients receive steps in one round-trip.
         */
        HabitDetail: {
            abandonability: components$1['schemas']['Abandonability'];
            /** @default false */
            active: boolean;
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            created_at: components$1['schemas']['Timestamp'];
            description?: string | null;
            /**
             * Format: int64
             * @default 0
             */
            display_id: number;
            end_time: components$1['schemas']['TimeOfDay'];
            /** @default false */
            fixed: boolean;
            id: string;
            /** @default false */
            parallelizable: boolean;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes: number;
            start_time: components$1['schemas']['TimeOfDay'];
            steps: components$1['schemas']['HabitStepRow'][];
            title: string;
            updated_at: components$1['schemas']['Timestamp'];
            /**
             * @description Window mode for generated tasks (#window_mode).
             *     `'day'` (default) = occurrence day's start_time..end_time.
             *     `'period'` = occurrence start_time .. next occurrence's start_time.
             * @default day
             */
            window_mode: components$1['schemas']['WindowMode'];
        };
        /** @description Request body for `POST /api/habits/{id}/estimate`. */
        HabitEstimateRequest: {
            /**
             * @description When true, persist the computed `avg_minutes` / `sigma_minutes` to the
             *     habit (and its steps). When false, return a preview only.
             * @default false
             */
            apply: boolean;
            /**
             * @description When true, detect and exclude outliers using the median absolute
             *     deviation (MAD) before computing the estimate.
             * @default false
             */
            detect_outliers: boolean;
        };
        /** @description Response from `POST /api/habits/{id}/estimate`. */
        HabitEstimateResult: {
            /** @description True when the result was written back to the habit/steps. */
            applied: boolean;
            /** Format: int64 */
            avg_minutes: number;
            /** Format: uint */
            excluded_count: number;
            /** @description The updated habit row, present only when `apply` was true. */
            habit?: components$1['schemas']['HabitRow'] | null;
            /** Format: uint */
            sample_count: number;
            /** @description Task-level samples for non-step habits. Empty for step-based habits. */
            samples: components$1['schemas']['HabitEstimateSample'][];
            /** Format: int64 */
            sigma_minutes: number;
            /** @description Per-step estimates for step-based habits. */
            steps: components$1['schemas']['HabitEstimateStep'][];
        };
        /** @description One completed task observation included in a habit estimate. */
        HabitEstimateSample: {
            /** Format: int64 */
            actual_minutes: number;
            excluded: boolean;
            task_id: string;
            title: string;
        };
        /** @description Estimate result for a single habit step. */
        HabitEstimateStep: {
            applied: boolean;
            /** Format: int64 */
            avg_minutes: number;
            /** Format: uint */
            excluded_count: number;
            /** Format: uint */
            sample_count: number;
            /** Format: int64 */
            sigma_minutes: number;
            step_id: string;
            title: string;
        };
        /**
         * @description Preview request for `POST /api/habits/preview`. Mirrors `CreateHabit`
         *     plus an optional step list and preview range.
         */
        HabitPreviewRequest: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            description?: string | null;
            end_time: components$1['schemas']['TimeOfDay'];
            fixed?: boolean | null;
            from?: string | null;
            /** Format: int64 */
            max_occurrences?: number | null;
            parallelizable?: boolean | null;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_time: components$1['schemas']['TimeOfDay'];
            /** @default [] */
            steps: components$1['schemas']['HabitStepInput'][];
            title: string;
            until?: string | null;
            /** @description Window mode: `'day'` or `'period'` (#window_mode). */
            window_mode?: components$1['schemas']['WindowMode'] | null;
        };
        /** @description A single task occurrence produced by `HabitPreviewRequest`. */
        HabitPreviewTask: {
            end_at: components$1['schemas']['Timestamp'];
            start_at: components$1['schemas']['Timestamp'];
            title: string;
        };
        HabitRow: {
            abandonability: components$1['schemas']['Abandonability'];
            /** @default false */
            active: boolean;
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            created_at: components$1['schemas']['Timestamp'];
            description?: string | null;
            /**
             * Format: int64
             * @default 0
             */
            display_id: number;
            end_time: components$1['schemas']['TimeOfDay'];
            /** @default false */
            fixed: boolean;
            id: string;
            /** @default false */
            parallelizable: boolean;
            recurrence: string;
            /** Format: int64 */
            sigma_minutes: number;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
            updated_at: components$1['schemas']['Timestamp'];
            /**
             * @description Window mode for generated tasks (#window_mode).
             *     `'day'` (default) = occurrence day's start_time..end_time.
             *     `'period'` = occurrence start_time .. next occurrence's start_time.
             * @default day
             */
            window_mode: components$1['schemas']['WindowMode'];
        };
        /**
         * @description A scheduled span for a habit (#503).
         *
         *     Its effect depends on `habits.active`:
         *     - `active = true`: the span suppresses task generation (a pause).
         *     - `active = false`: the span enables task generation (an activation window).
         *
         *     `start_date` / `end_date` are inclusive `YYYY-MM-DD` strings in the
         *     user's local timezone.
         */
        HabitScheduledSpanRow: {
            created_at: components$1['schemas']['Timestamp'];
            end_date: components$1['schemas']['Date'];
            habit_id: string;
            id: string;
            reason?: string | null;
            start_date: components$1['schemas']['Date'];
        };
        /**
         * @description Input element for `PUT /api/habits/:id/steps` (bulk replace, #95).
         *     An `id` present in the DB keeps the existing step (preserving its link to
         *     generated tasks); an `id` absent or unknown creates a new step. Existing
         *     steps not in the array are deleted. `depends_on` references step ids that
         *     must exist in the resulting set.
         */
        HabitStepInput: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes: number;
            /** @default [] */
            depends_on: string[];
            description?: string | null;
            end_time: components$1['schemas']['TimeOfDay'];
            fixed?: boolean | null;
            id?: string | null;
            parallelizable?: boolean | null;
            /** Format: int64 */
            position: number;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
        };
        /**
         * @description A step of a multi-step habit (#95). Each step produces one task per
         *     occurrence with its own window / cost / flags. Steps form a DAG via
         *     `depends_on` (JSON array of step ids within the same habit).
         */
        HabitStepRow: {
            abandonability: components$1['schemas']['Abandonability'];
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            created_at: components$1['schemas']['Timestamp'];
            /**
             * @description JSON array of step ids this step depends on (within the same habit).
             * @default []
             */
            depends_on: components$1['schemas']['JsonString'];
            description?: string | null;
            end_time: components$1['schemas']['TimeOfDay'];
            /** @default false */
            fixed: boolean;
            habit_id: string;
            id: string;
            /** @default false */
            parallelizable: boolean;
            /** Format: int64 */
            position: number;
            /** Format: int64 */
            sigma_minutes: number;
            start_time: components$1['schemas']['TimeOfDay'];
            title: string;
        };
        HealthCheckResponse: {
            status: string;
        };
        IcalImportResult: {
            /** Format: uint */
            imported: number;
            task_ids: string[];
        };
        /**
         * @description A wrapper that serializes `T` as a JSON string on the wire and in the DB.
         *
         *     Common instantiations:
         *     - `JsonString<Vec<String>>` — task / step dependency lists
         *     - `JsonString<Vec<ScheduleEntry>>` — schedule entry arrays
         */
        JsonString: string;
        /**
         * @description A wrapper that serializes `T` as a JSON string on the wire and in the DB.
         *
         *     Common instantiations:
         *     - `JsonString<Vec<String>>` — task / step dependency lists
         *     - `JsonString<Vec<ScheduleEntry>>` — schedule entry arrays
         */
        JsonString2: string;
        /**
         * @description Query for the memory read auto-injection retrieval path (WI-4 / #1003).
         *
         *     Unlike [`MemoryQuery`] (user-facing keyword search), this is a *reverse*
         *     lookup: memories whose `normalized_key` occurs as a substring of `text`
         *     are candidates, ranked server-side by key specificity and recency. Used at
         *     turn start to surface `proper_noun` / `fact` memories without the agent
         *     calling any search tool.
         */
        MemoryInjectionQuery: {
            /**
             * Format: uint32
             * @description Maximum number of memories to return (default 5, capped at 20).
             */
            limit?: number | null;
            /** @description The raw user utterance. Normalized server-side before matching. */
            text: string;
        };
        /** @description Result of a memory read auto-injection retrieval (WI-4 / #1003). */
        MemoryInjectionResult: {
            /**
             * @description Per-kind memory counts so the agent knows whether the store is
             *     non-empty even when no memory matches the utterance.
             */
            counts: components$1['schemas']['MemoryKindCounts'];
            /** @description Matching `proper_noun` / `fact` memories, ranked for injection. */
            memories: components$1['schemas']['MemoryRow'][];
        };
        /** @enum {string} */
        MemoryKind: 'proper_noun' | 'fact';
        /** @description Total memory rows per kind, used for the system-prompt memory hint. */
        MemoryKindCounts: {
            /** Format: int64 */
            fact: number;
            /** Format: int64 */
            proper_noun: number;
        };
        MemoryQuery: {
            kind?: components$1['schemas']['MemoryKind'] | null;
            /** Format: int64 */
            limit?: number | null;
            q: string;
            subject_id?: string | null;
            subject_type?: components$1['schemas']['SubjectType'] | null;
        };
        MemoryRow: {
            content: string;
            created_at: components$1['schemas']['Timestamp'];
            id: string;
            key: string;
            kind: components$1['schemas']['MemoryKind'];
            last_used_at?: components$1['schemas']['Timestamp'] | null;
            normalized_content?: string;
            normalized_key?: string;
            /** Format: int64 */
            revision: number;
            /** @default user_confirmed */
            source: components$1['schemas']['MemorySource'];
            subject_id: string;
            /** @default  */
            subject_type: components$1['schemas']['SubjectType'];
            updated_at: components$1['schemas']['Timestamp'];
        };
        /**
         * @description Provenance of a memory row (see `doc/code-quality-issues.md` §34).
         *
         *     Matches the `memories.source` CHECK constraint in
         *     `migrations/016_memory.sql`: `user_confirmed`, `agent_inferred`,
         *     `imported`. Currently only `UserConfirmed` is written by the storage
         *     layer, but the schema accepts all three so the enum keeps them as
         *     variants to round-trip existing rows safely.
         * @enum {string}
         */
        MemorySource: 'user_confirmed' | 'agent_inferred' | 'imported';
        /** @description Request body for `PATCH /api/schedule/entries/:task_id`. */
        MoveEntry: {
            /** @default false */
            force: boolean;
            start_at: components$1['schemas']['Timestamp'];
        };
        /** @description Response body for `PATCH /api/schedule/entries/:task_id`. */
        MoveEntryResponse: {
            end_at: components$1['schemas']['Timestamp'];
            start_at: components$1['schemas']['Timestamp'];
            task_id: string;
            /** @default [] */
            warnings: string[];
        };
        OAuthCallbackRequest: {
            code: string;
            redirect_uri?: string | null;
        };
        /** @description Response for `POST /api/sync/oauth/callback`. */
        OAuthCallbackResponse: {
            refresh_token_set: boolean;
        };
        OAuthUrlRequest: {
            redirect_uri: string;
        };
        /** @description Response for `POST /api/sync/oauth/url`. */
        OAuthUrlResponse: {
            url: string;
        };
        ProgressEventRow: {
            /** Format: int64 */
            active_minutes: number;
            at: components$1['schemas']['Timestamp'];
            /** Format: int64 */
            delta_quantity?: number | null;
            id: string;
            note?: string | null;
            quantity_done?: components$1['schemas']['Quantity'] | null;
            task_id?: string | null;
            work_session_id: string;
        };
        /**
         * Format: int64
         * @description A non-negative integer quantity.
         *
         *     Negative values are rejected at construction time. The inner `i64` can be
         *     retrieved via [`Quantity::get`].
         */
        Quantity: number;
        RecordWorkSessionProgress: {
            note?: string | null;
            quantity_done: components$1['schemas']['Quantity'];
            quantity_total?: components$1['schemas']['Quantity'] | null;
        };
        /**
         * @description A redundant (composite / transitively implied) dependency edge with a
         *     witness path proving the direct edge is unnecessary (#355).
         */
        RedundantDependency: {
            from: string;
            from_title: string;
            to: string;
            to_title: string;
            /** @description Witness path `from → … → to` (endpoints included, length >= 3). */
            via: components$1['schemas']['DependencyNode'][];
        };
        /** @description Request body for `POST /api/schedule/reschedule`. */
        Reschedule: {
            from?: string | null;
            mode: components$1['schemas']['ScheduleMode'];
            /** @default [] */
            pinned: string[];
            /** @default recommended */
            sleep: components$1['schemas']['SleepInput'];
            task_ids?: string[] | null;
            until?: string | null;
        };
        SaveScheduleRequest: {
            entries: components$1['schemas']['ScheduleEntry'][];
            /** @default [] */
            horizon_task_ids: string[];
            /** @default [] */
            mark_scheduled_task_ids: string[];
        };
        ScheduleEntry: {
            end_at: components$1['schemas']['Timestamp'];
            start_at: components$1['schemas']['Timestamp'];
            task_id: string;
        };
        /**
         * @description Reschedule / preview mode for schedule operations.
         *
         *     `Range` replans tasks within a time window; `Tasks` replans a specific
         *     set of task IDs; `Full` regenerates the entire schedule (only valid for
         *     preview — `reschedule` rejects it). Used by `Reschedule`,
         *     `SchedulePreviewRequest`, and the CLI's `ScheduleCommands::Reschedule`.
         * @enum {string}
         */
        ScheduleMode: 'range' | 'tasks' | 'full';
        /** @description Request body for `POST /api/schedule/preview`. */
        SchedulePreviewRequest: {
            from?: string | null;
            /** @default full */
            mode: components$1['schemas']['ScheduleMode'];
            /** @default [] */
            pinned: string[];
            /** @default recommended */
            sleep: components$1['schemas']['SleepInput'];
            task_ids?: string[] | null;
            until?: string | null;
        };
        /** @description Response body for `POST /api/schedule/preview`. */
        SchedulePreviewResponse: {
            /** @default [] */
            displaced_task_ids: string[];
            entries: components$1['schemas']['ScheduleEntry'][];
            /**
             * Format: int64
             * @default 0
             */
            sleep_minutes_after: number;
            /**
             * Format: int64
             * @default 0
             */
            sleep_minutes_before: number;
            /** @default [] */
            unscheduled_task_ids: string[];
            /** @default [] */
            warnings: string[];
        };
        ScheduleRevisionResponse: {
            /** Format: int64 */
            revision: number;
        };
        ScheduleRow: {
            created_at: components$1['schemas']['Timestamp'];
            /** @default [] */
            horizon_task_ids: components$1['schemas']['JsonString'];
            id: string;
            /** @default [] */
            schedule: components$1['schemas']['JsonString2'];
            updated_at: components$1['schemas']['Timestamp'];
        };
        SettingsRow: {
            /**
             * 459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
             * Format: int64
             */
            comfortable_minutes?: number | null;
            created_at: components$1['schemas']['Timestamp'];
            id: string;
            /**
             * 459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
             * Format: int64
             */
            maximum_minutes?: number | null;
            /**
             * Format: int64
             * @description スケジュール計画の期間（日数）。horizon 計算に使う。デフォルト 14。
             * @default 14
             */
            plan_length_days: number;
            /**
             * Format: int64
             * @description 乱数シード。`None` の場合は決定的なデフォルト。
             * @default null
             */
            seed: number | null;
            sleep_end: components$1['schemas']['TimeOfDay'];
            sleep_start: components$1['schemas']['TimeOfDay'];
            /**
             * @description 使用する solver。`"sa"` / `"priority"` / `"auto"`。未設定の場合は `sa`。未知値はエラー。
             * @default sa
             */
            solver: components$1['schemas']['Solver'];
            /**
             * Format: int64
             * @description 求解時間の上限（ミリ秒）。`None` または `0` の場合は制限なし。
             * @default null
             */
            time_budget_ms: number | null;
            tz: string;
            updated_at: components$1['schemas']['Timestamp'];
            /**
             * @description 前回スケジュールから priority/ALNS の初期解を warm start する。
             * @default false
             */
            warm_start: boolean;
        };
        SimilarTaskQuery: {
            /** Format: int64 */
            limit?: number | null;
            q: string;
        };
        SimilarTaskRow: {
            /** Format: int64 */
            actual_minutes?: number | null;
            /** Format: int64 */
            avg_minutes: number;
            completed_at?: components$1['schemas']['Timestamp'] | null;
            /** Format: int64 */
            display_id: number;
            /** Format: int64 */
            sigma_minutes: number;
            /**
             * @default {
             *       "metric": "dice",
             *       "score": 0
             *     }
             */
            similarity: components$1['schemas']['Similarity'];
            task_id: string;
            title: string;
            updated_at?: components$1['schemas']['Timestamp'];
        };
        /**
         * @description A similarity score pairing a metric with a numeric value.
         *
         *     Serialized as `{"metric":"dice","score":0.85}`. Implements [`Default`]
         *     (`Dice`, `0.0`) so it can be used with `#[sqlx(skip)]` / `#[serde(default)]`
         *     on row structs where the SQL/JSON result does not include the field.
         */
        Similarity: {
            metric: components$1['schemas']['SimilarityMetric'];
            /** Format: double */
            score: number;
        };
        /**
         * @description Similarity metric used by `Similarity` (see `doc/code-quality-issues.md` #33).
         *
         *     Kept in `takusu-types` so `takusu-contracts`, `takusu-client`, and
         *     `takusu-worker` can all use it without changing the crate dependency
         *     graph.
         * @enum {string}
         */
        SimilarityMetric: 'dice';
        SkillRow: {
            body: string;
            /** @default false */
            built_in: boolean;
            created_at: components$1['schemas']['Timestamp'];
            description: string;
            name: string;
            slug: string;
            updated_at: components$1['schemas']['Timestamp'];
        };
        /**
         * @description Sleep configuration for schedule generation / reschedule / preview.
         *
         *     Parsed at the API/CLI boundary from a plain string; consumed by
         *     `takusu-local-lib` to build a `SleepConfig` using settings + timezone.
         */
        SleepInput: string;
        /**
         * @description Phase 5 solver label (see `doc/type-safety-issues.md` §3.3).
         *
         *     Kept in `takusu-types` because `takusu-core` does not depend on `serde`,
         *     and `takusu-contracts` / `takusu-client` / `takusu-worker` all depend on `takusu-types`.
         * @enum {string}
         */
        Solver: 'sa' | 'priority' | 'auto';
        SplitResult: {
            original: components$1['schemas']['TaskRow'];
            remainder: components$1['schemas']['TaskRow'];
        };
        SplitTask: {
            /** @description Optional description for the remainder. */
            description?: string | null;
            /** @description Optional deadline for the remainder (defaults to the original end_at). */
            end_at?: components$1['schemas']['Timestamp'] | null;
            /** @description Quantity to keep on the original task. */
            retained_quantity: components$1['schemas']['Quantity'];
            /** @description If true, make the remainder depend on the original task. */
            set_dependency?: boolean | null;
            /** @description Optional title for the remainder (defaults to the original title). */
            title?: string | null;
        };
        StartWorkSession: {
            note?: string | null;
            quantity_total?: components$1['schemas']['Quantity'] | null;
            quantity_unit?: string | null;
            task_id?: string | null;
            title?: string | null;
        };
        /** @enum {string} */
        SubjectType: '' | 'task' | 'habit' | 'skill' | 'schedule';
        /** @description Response for `POST /api/sync/trigger`. */
        SyncTriggerResponse: {
            status: string;
        };
        TaskQueryParams: {
            from?: string | null;
            habit_id?: string | null;
            ical_uid?: string | null;
            /** Format: int64 */
            limit?: number | null;
            no_overdue?: boolean | null;
            q?: string | null;
            status?: string | null;
            until?: string | null;
        };
        TaskRow: {
            abandonability: components$1['schemas']['Abandonability'];
            /**
             * Format: int64
             * @description Total active work minutes from work_sessions (NULL when no work has been done).
             */
            actual_minutes?: number | null;
            /** @default false */
            allows_parallel: boolean;
            /** Format: int64 */
            avg_minutes: number;
            /** @description WI-9: wall-clock completion time, set by `complete`. */
            completed_at?: components$1['schemas']['Timestamp'] | null;
            created_at: components$1['schemas']['Timestamp'];
            /** @default [] */
            depends: components$1['schemas']['JsonString'];
            description?: string | null;
            /**
             * Format: int64
             * @default 0
             */
            display_id: number;
            end_at: components$1['schemas']['Timestamp'];
            /** @default false */
            fixed: boolean;
            habit_id?: string | null;
            /**
             * @description The habit step that generated this task, if any (#95). NULL for simple
             *     (step-less) habits and manually created tasks.
             */
            habit_step_id?: string | null;
            ical_uid?: string | null;
            id: string;
            /** @description WI-9: pre-split total quantity, kept for lineage. */
            original_quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @default false */
            parallelizable: boolean;
            /**
             * @description WI-9: quantity already done. Defaults to 0.
             * @default 0
             */
            quantity_done: components$1['schemas']['Quantity'];
            /** @description WI-9: total quantity for a quantitative task (e.g. 30 題). */
            quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: unit for the quantity (e.g. "題"). */
            quantity_unit?: string | null;
            /** Format: int64 */
            sigma_minutes: number;
            /** @description WI-9: for a remainder task, the id of the task it was split from. */
            split_from_task_id?: string | null;
            start_at?: components$1['schemas']['Timestamp'] | null;
            status: components$1['schemas']['TaskStatus'];
            title: string;
            updated_at: components$1['schemas']['Timestamp'];
            /** @default false */
            user_edited: boolean;
        };
        /**
         * @description Phase 1 type-safe labels (see `doc/type-safety-issues.md` §3.1 / 3.2 / 3.5 / 3.6).
         *
         *     These are intentionally kept in `takusu-types` so that `takusu-contracts`,
         *     `takusu-client`, and `takusu-worker` can all use them without changing the
         *     crate dependency graph.
         * @enum {string}
         */
        TaskStatus: 'pending' | 'scheduled' | 'in_progress' | 'completed' | 'skipped';
        /**
         * @description A time of day in `HH:MM` format with minutes snapped to 5-minute slots.
         *
         *     Serialized as a `"HH:MM"` string for JSON and stored as `TEXT` in SQLite.
         *     This type was originally defined in `takusu-habit` and has been moved here
         *     so that `takusu-contracts` / `takusu-client` / `takusu-worker` can use it
         *     without depending on `takusu-habit`.
         */
        TimeOfDay: string;
        /**
         * @description An RFC 3339 timestamp.
         *
         *     Serialized as an RFC 3339 string for JSON and stored as `TEXT` in SQLite.
         *     Wraps [`jiff::Timestamp`].
         */
        Timestamp: string;
        TokenCreateResponse: {
            created_at: components$1['schemas']['Timestamp'];
            expires_at?: components$1['schemas']['Timestamp'] | null;
            /** Format: int64 */
            id: number;
            label?: string | null;
            scope: components$1['schemas']['TokenScope'];
            token: string;
        };
        TokenRow: {
            created_at: components$1['schemas']['Timestamp'];
            created_by: string;
            expires_at?: components$1['schemas']['Timestamp'] | null;
            /** Format: int64 */
            id: number;
            jti: string;
            label?: string | null;
            revoked_at?: components$1['schemas']['Timestamp'] | null;
            scope: components$1['schemas']['TokenScope'];
        };
        /** @enum {string} */
        TokenScope: 'read-write' | 'root';
        /** @description An unresolved elapsed-time interval that needs settlement (WI-10 / WI-18). */
        UnsettledIntervalRow: {
            classification: string;
            created_at: components$1['schemas']['Timestamp'];
            end_at: components$1['schemas']['Timestamp'];
            id: string;
            operation_id?: string | null;
            settled_at?: components$1['schemas']['Timestamp'] | null;
            source: string;
            start_at: components$1['schemas']['Timestamp'];
        };
        UpdateGoogleCalSettings: {
            calendar_id?: string | null;
            client_id?: string | null;
            client_secret?: string | null;
            /**
             * Format: int64
             * @description `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
             */
            color_id?: number | null;
            enabled?: boolean | null;
            refresh_token?: string | null;
            /**
             * Format: int64
             * @description `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
             */
            reminder_minutes?: number | null;
            /** @description `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。 */
            transparency?: string | null;
            /** @description `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。 */
            visibility?: string | null;
        };
        UpdateHabit: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            active?: boolean | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes?: number | null;
            description?: string | null;
            end_time?: components$1['schemas']['TimeOfDay'] | null;
            fixed?: boolean | null;
            parallelizable?: boolean | null;
            recurrence?: string | null;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_time?: components$1['schemas']['TimeOfDay'] | null;
            title?: string | null;
            /** @description Window mode: `'day'` or `'period'` (#window_mode). */
            window_mode?: components$1['schemas']['WindowMode'] | null;
        };
        UpdateMemory: {
            content?: string | null;
            /** Format: int64 */
            observed_revision: number;
        };
        UpdateSettings: {
            /**
             * 459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
             * Format: int64
             */
            comfortable_minutes?: number | null;
            /**
             * 459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
             * Format: int64
             */
            maximum_minutes?: number | null;
            /**
             * Format: int64
             * @description スケジュール計画の期間（日数）。
             */
            plan_length_days?: number | null;
            /**
             * Format: int64
             * @description 乱数シード。`None` でデフォルト。
             */
            seed?: number | null;
            sleep_end?: components$1['schemas']['TimeOfDay'] | null;
            sleep_start?: components$1['schemas']['TimeOfDay'] | null;
            /** @description 使用する solver。`"sa"` / `"priority"` / `"auto"`。 */
            solver?: components$1['schemas']['Solver'] | null;
            /**
             * Format: int64
             * @description 求解時間の上限（ミリ秒）。`None` または `0` で制限なし。
             */
            time_budget_ms?: number | null;
            tz?: string | null;
            /** @description 前回スケジュールから priority/ALNS の初期解を warm start する。 */
            warm_start?: boolean | null;
        };
        UpdateSkill: {
            body?: string | null;
            description?: string | null;
            name?: string | null;
        };
        UpdateTask: {
            abandonability?: components$1['schemas']['Abandonability'] | null;
            allows_parallel?: boolean | null;
            /** Format: int64 */
            avg_minutes?: number | null;
            depends?: string[] | null;
            description?: string | null;
            end_at?: components$1['schemas']['Timestamp'] | null;
            fixed?: boolean | null;
            habit_id?: string | null;
            habit_step_id?: string | null;
            /** @description WI-9: pre-split total quantity, kept for lineage. */
            original_quantity_total?: components$1['schemas']['Quantity'] | null;
            parallelizable?: boolean | null;
            /** @description WI-9: quantity already done. */
            quantity_done?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: total quantity for a quantitative task. */
            quantity_total?: components$1['schemas']['Quantity'] | null;
            /** @description WI-9: unit for the quantity. */
            quantity_unit?: string | null;
            /** Format: int64 */
            sigma_minutes?: number | null;
            start_at?: components$1['schemas']['Timestamp'] | null;
            status?: components$1['schemas']['TaskStatus'] | null;
            title?: string | null;
            user_edited?: boolean | null;
        };
        UpdateWorkersConfig: {
            token: string;
            url: string;
        };
        /** @enum {string} */
        WindowMode: 'day' | 'period';
        WorkSessionListQuery: {
            task_id?: string | null;
        };
        WorkSessionProgressResult: {
            estimator?: components$1['schemas']['EstimatorResult'] | null;
            /**
             * @description The recorded event, or `None` when the reported quantity_done has not
             *     changed (no-op).
             */
            event?: components$1['schemas']['ProgressEventRow'] | null;
            /**
             * @description True when the reported quantity_done reaches or exceeds the task total.
             * @default false
             */
            suggests_completion: boolean;
            task?: components$1['schemas']['TaskRow'] | null;
            work_session: components$1['schemas']['WorkSessionRow'];
        };
        /**
         * @description A top-level work session. It may be linked to a task, or it may be a
         *     standalone session that is later converted into a task.
         */
        WorkSessionRow: {
            created_at: components$1['schemas']['Timestamp'];
            ended_at?: components$1['schemas']['Timestamp'] | null;
            id: string;
            note?: string | null;
            quantity_done: components$1['schemas']['Quantity'];
            quantity_total?: components$1['schemas']['Quantity'] | null;
            quantity_unit?: string | null;
            started_at: components$1['schemas']['Timestamp'];
            task_id?: string | null;
            title?: string | null;
        };
        /** @description Response for `PUT /api/workers/config`. */
        WorkersConfigUpdateResponse: {
            ok: boolean;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}

type components = components$1;
type S = components['schemas'];
type TaskStatus = S['TaskStatus'];
type TaskRow = S['TaskRow'];
type CreateTask = S['CreateTask'];
type UpdateTask = S['UpdateTask'];
type Completion = S['Completion'];
type HabitRow = S['HabitRow'];
type CreateHabit = S['CreateHabit'];
type UpdateHabit = S['UpdateHabit'];
type HabitScheduledSpanRow = S['HabitScheduledSpanRow'];
type CreateHabitScheduledSpan = S['CreateHabitScheduledSpan'];
type HabitStepRow = S['HabitStepRow'];
type HabitStepInput = S['HabitStepInput'];
type HabitDetail = S['HabitDetail'];
type HabitEstimateRequest = S['HabitEstimateRequest'];
type HabitEstimateSample = S['HabitEstimateSample'];
type HabitEstimateStep = S['HabitEstimateStep'];
type HabitEstimateResult = S['HabitEstimateResult'];
type HabitPreviewRequest = S['HabitPreviewRequest'];
type HabitPreviewTask = S['HabitPreviewTask'];
type ScheduleEntry = S['ScheduleEntry'];
type ScheduleRow = S['ScheduleRow'];
type GenerateSchedule = S['GenerateSchedule'];
type SettingsRow = S['SettingsRow'];
type UpdateSettings = S['UpdateSettings'];
type TokenRow = S['TokenRow'];
type TokenCreateResponse = S['TokenCreateResponse'];
type SkillRow = S['SkillRow'];
type CreateSkill = S['CreateSkill'];
type UpdateSkill = S['UpdateSkill'];
type DeleteAllGcalFailure = S['DeleteAllGcalFailure'];
type IcalImportResult = S['IcalImportResult'];
type DependencyNode = S['DependencyNode'];
type RedundantDependency = S['RedundantDependency'];
type DependencyAnalysisResponse = S['DependencyAnalysisResponse'];
type StartWorkSession = S['StartWorkSession'];
type AttachWorkSession = S['AttachWorkSession'];
type ConvertWorkSession = S['ConvertWorkSession'];
type RecordWorkSessionProgress = S['RecordWorkSessionProgress'];
type WorkSessionRow = S['WorkSessionRow'];
type WorkSessionProgressResult = S['WorkSessionProgressResult'];
type ProgressEventRow = S['ProgressEventRow'];
type SplitTask = S['SplitTask'];
type SplitResult = S['SplitResult'];
type CommentRow = S['CommentRow'];
type CreateComment = S['CreateComment'];
type CommentAuthor = S['CommentAuthor'];
type EvaluationInputs = S['EvaluationInputs'];
type CoverageState = S['CoverageState'];
type CoverageEvaluation = S['CoverageEvaluation'];
type SyncTriggerResponse = S['SyncTriggerResponse'];
type OAuthCallbackResponse = S['OAuthCallbackResponse'];
type WindowMode = S['WindowMode'];
type TaskQuery = S['TaskQueryParams'];
type RescheduleRequest = S['Reschedule'];
type MoveEntryRequest = S['MoveEntry'];
type MoveEntryResponse = S['MoveEntryResponse'];
type GoogleCalSettings = S['GoogleCalSettingsOutput'];
type DeleteAllGcalResponse = S['DeleteAllGcalResult'];
type GoogleCalEventMapping = S['GoogleCalEventRow'];
type UpdateGoogleCalSettings = S['UpdateGoogleCalSettings'];
declare function parseDepends(depends: string): string[];
declare function parseDependsOn(dependsOn: string): string[];
declare const WINDOW_MODE_DAY: "day";
declare const WINDOW_MODE_PERIOD: "period";
declare function parseSchedule(schedule: string): ScheduleEntry[];
declare function parseHorizonTaskIds(raw: string | undefined | null): Set<string>;

declare class ApiError extends Error {
    status: number;
    body: string;
    constructor(status: number, body: string);
}
declare class TakusuClient {
    readonly baseUrl: string;
    private token;
    constructor(baseUrl: string, token: string);
    private request;
    health(signal?: AbortSignal): Promise<string>;
    listTasks(query?: TaskQuery): Promise<TaskRow[]>;
    completeTaskQuery(q: string, limit?: number): Promise<Completion[]>;
    getTask(id: string): Promise<TaskRow>;
    createTask(body: CreateTask): Promise<TaskRow>;
    updateTask(id: string, body: UpdateTask): Promise<TaskRow>;
    replaceTask(id: string, body: CreateTask): Promise<TaskRow>;
    deleteTask(id: string): Promise<void>;
    listComments(taskId: string): Promise<CommentRow[]>;
    createComment(taskId: string, body: CreateComment, operationId?: string): Promise<CommentRow>;
    createAgentComment(taskId: string, body: CreateComment, operationId?: string): Promise<CommentRow>;
    deleteComment(id: string): Promise<void>;
    createWorkSession(body: StartWorkSession, operationId?: string): Promise<WorkSessionRow>;
    listWorkSessions(taskId?: string): Promise<WorkSessionRow[]>;
    getWorkSession(id: string): Promise<WorkSessionRow>;
    pauseWorkSession(id: string, operationId?: string): Promise<WorkSessionRow>;
    completeWorkSession(id: string, operationId?: string): Promise<WorkSessionRow>;
    recordWorkSessionProgress(id: string, body: RecordWorkSessionProgress, operationId?: string): Promise<WorkSessionProgressResult>;
    attachWorkSession(id: string, body: AttachWorkSession, operationId?: string): Promise<WorkSessionRow>;
    convertWorkSession(id: string, body: ConvertWorkSession, operationId?: string): Promise<TaskRow>;
    splitTask(id: string, body: SplitTask, operationId?: string): Promise<SplitResult>;
    analyzeTaskDependencies(): Promise<DependencyAnalysisResponse>;
    importIcal(icalText: string): Promise<IcalImportResult>;
    listHabits(): Promise<HabitRow[]>;
    getHabit(id: string): Promise<HabitDetail>;
    estimateHabit(id: string, body: HabitEstimateRequest): Promise<HabitEstimateResult>;
    createHabit(body: CreateHabit): Promise<HabitRow>;
    previewHabit(body: HabitPreviewRequest): Promise<HabitPreviewTask[]>;
    updateHabit(id: string, body: UpdateHabit): Promise<HabitRow>;
    replaceHabit(id: string, body: CreateHabit): Promise<HabitRow>;
    deleteHabit(id: string): Promise<void>;
    listHabitScheduledSpans(id: string): Promise<HabitScheduledSpanRow[]>;
    listAllHabitScheduledSpans(): Promise<HabitScheduledSpanRow[]>;
    createHabitScheduledSpan(id: string, body: CreateHabitScheduledSpan): Promise<HabitScheduledSpanRow>;
    deleteHabitScheduledSpan(id: string, spanId: string): Promise<void>;
    listHabitSteps(id: string): Promise<HabitStepRow[]>;
    listAllHabitSteps(): Promise<HabitStepRow[]>;
    replaceHabitSteps(id: string, steps: HabitStepInput[]): Promise<HabitStepRow[]>;
    analyzeHabitStepDependencies(id: string): Promise<DependencyAnalysisResponse>;
    getSchedule(): Promise<ScheduleRow>;
    generateSchedule(body: GenerateSchedule): Promise<ScheduleRow>;
    reschedule(body: RescheduleRequest): Promise<ScheduleRow>;
    moveEntry(taskId: string, body: MoveEntryRequest): Promise<MoveEntryResponse>;
    clearSchedule(): Promise<void>;
    getSettings(): Promise<SettingsRow>;
    updateSettings(body: UpdateSettings): Promise<SettingsRow>;
    listTokens(): Promise<TokenRow[]>;
    createToken(label?: string): Promise<TokenCreateResponse>;
    revokeToken(id: number): Promise<void>;
    getGcalSettings(): Promise<GoogleCalSettings>;
    oauthCallback(code: string, redirectUri?: string): Promise<OAuthCallbackResponse>;
    updateGcalSettings(body: UpdateGoogleCalSettings): Promise<GoogleCalSettings>;
    triggerSync(): Promise<SyncTriggerResponse>;
    deleteAllGcalEvents(): Promise<DeleteAllGcalResponse>;
    listGcalMappings(): Promise<GoogleCalEventMapping[]>;
    listSkills(): Promise<SkillRow[]>;
    getSkill(slug: string): Promise<SkillRow>;
    createSkill(body: CreateSkill): Promise<SkillRow>;
    updateSkill(slug: string, body: UpdateSkill): Promise<SkillRow>;
    deleteSkill(slug: string): Promise<void>;
    workerHealthCheck(): Promise<{
        status: string;
    }>;
    updateWorkersConfig(body: {
        url: string;
        token: string;
    }): Promise<{
        ok: boolean;
    }>;
    getEvaluationSnapshot(): Promise<EvaluationInputs>;
}

export { ApiError, type AttachWorkSession, type CommentAuthor, type CommentRow, type Completion, type ConvertWorkSession, type CoverageEvaluation, type CoverageState, type CreateComment, type CreateHabit, type CreateHabitScheduledSpan, type CreateSkill, type CreateTask, type DeleteAllGcalFailure, type DeleteAllGcalResponse, type DependencyAnalysisResponse, type DependencyNode, type EvaluationInputs, type GenerateSchedule, type GoogleCalEventMapping, type GoogleCalSettings, type HabitDetail, type HabitEstimateRequest, type HabitEstimateResult, type HabitEstimateSample, type HabitEstimateStep, type HabitPreviewRequest, type HabitPreviewTask, type HabitRow, type HabitScheduledSpanRow, type HabitStepInput, type HabitStepRow, type IcalImportResult, type MoveEntryRequest, type MoveEntryResponse, type OAuthCallbackResponse, type ProgressEventRow, type RecordWorkSessionProgress, type RedundantDependency, type RescheduleRequest, type ScheduleEntry, type ScheduleRow, type SettingsRow, type SkillRow, type SplitResult, type SplitTask, type StartWorkSession, type SyncTriggerResponse, TakusuClient, type TaskQuery, type TaskRow, type TaskStatus, type TokenCreateResponse, type TokenRow, type UpdateGoogleCalSettings, type UpdateHabit, type UpdateSettings, type UpdateSkill, type UpdateTask, WINDOW_MODE_DAY, WINDOW_MODE_PERIOD, type WindowMode, type WorkSessionProgressResult, type WorkSessionRow, type components, parseDepends, parseDependsOn, parseHorizonTaskIds, parseSchedule };
