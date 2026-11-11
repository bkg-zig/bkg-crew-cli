-- Initial schema for bkgcrewcli task queue
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY NOT NULL,
    repo_root TEXT NOT NULL,
    name TEXT NOT NULL,
    task_type INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 1,
    instructions_file TEXT NOT NULL,
    agent TEXT NOT NULL,
    status INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    branch_name TEXT NOT NULL,
    error_message TEXT,
    blocked_reason TEXT,
    source_commit TEXT NOT NULL,
    source_branch TEXT,
    stack TEXT NOT NULL DEFAULT 'default',
    project TEXT NOT NULL DEFAULT 'default',
    copied_repo_path TEXT,
    parent_ids TEXT NOT NULL DEFAULT '[]',
    child_ids TEXT NOT NULL DEFAULT '[]',
    interactive INTEGER NOT NULL DEFAULT 0,
    sandbox_config TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    timeout_seconds INTEGER NOT NULL DEFAULT 3600,
    tags TEXT NOT NULL DEFAULT '[]',
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_agent ON tasks(agent);
CREATE INDEX IF NOT EXISTS idx_tasks_created_at ON tasks(created_at);
CREATE INDEX IF NOT EXISTS idx_tasks_branch ON tasks(branch_name);
CREATE INDEX IF NOT EXISTS idx_tasks_parent ON tasks(parent_ids);

-- Task logs table
CREATE TABLE IF NOT EXISTS task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    level INTEGER NOT NULL DEFAULT 2,
    message TEXT NOT NULL,
    source INTEGER NOT NULL DEFAULT 2,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_logs_task_id ON task_logs(task_id);
CREATE INDEX IF NOT EXISTS idx_task_logs_timestamp ON task_logs(timestamp);

-- Task metrics table
CREATE TABLE IF NOT EXISTS task_metrics (
    task_id TEXT PRIMARY KEY NOT NULL,
    duration_seconds INTEGER,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    api_calls INTEGER NOT NULL DEFAULT 0,
    sandbox_starts INTEGER NOT NULL DEFAULT 0,
    retries INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

-- Sandboxes table
CREATE TABLE IF NOT EXISTS sandboxes (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT,
    template_id TEXT NOT NULL,
    status INTEGER NOT NULL DEFAULT 0,
    endpoint TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    started_at TEXT,
    stopped_at TEXT,
    cpu_limit TEXT,
    memory_limit TEXT,
    network_mode INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_sandboxes_task_id ON sandboxes(task_id);
CREATE INDEX IF NOT EXISTS idx_sandboxes_status ON sandboxes(status);

-- Sandbox resource usage history
CREATE TABLE IF NOT EXISTS sandbox_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sandbox_id TEXT NOT NULL,
    timestamp TEXT NOT NULL DEFAULT (datetime('now')),
    cpu_percent REAL NOT NULL,
    memory_mb INTEGER NOT NULL,
    disk_mb INTEGER NOT NULL,
    network_rx_mb INTEGER NOT NULL DEFAULT 0,
    network_tx_mb INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (sandbox_id) REFERENCES sandboxes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_sandbox_metrics_sandbox_id ON sandbox_metrics(sandbox_id);
CREATE INDEX IF NOT EXISTS idx_sandbox_metrics_timestamp ON sandbox_metrics(timestamp);

-- Crew workflows table
CREATE TABLE IF NOT EXISTS crews (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Crew tasks (many-to-many)
CREATE TABLE IF NOT EXISTS crew_tasks (
    crew_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (crew_id, task_id),
    FOREIGN KEY (crew_id) REFERENCES crews(id) ON DELETE CASCADE,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_crew_tasks_crew_id ON crew_tasks(crew_id);
CREATE INDEX IF NOT EXISTS idx_crew_tasks_task_id ON crew_tasks(task_id);

-- Notifications table
CREATE TABLE IF NOT EXISTS notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    task_id TEXT,
    sandbox_id TEXT,
    read INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL,
    FOREIGN KEY (sandbox_id) REFERENCES sandboxes(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_notifications_task_id ON notifications(task_id);
CREATE INDEX IF NOT EXISTS idx_notifications_read ON notifications(read);
CREATE INDEX IF NOT EXISTS idx_notifications_created_at ON notifications(created_at);