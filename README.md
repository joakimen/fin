# fin

List completed tasks from different systems.

## Description

fin collects tasks from different sources, such as Jira and Todoist, etc. and displas the results in a single list.

## Installation

```bash
$ go install github.com/joakimen/fin
```

## Usage

For the example below, assume the current time is `2024-02-23 17:00`.

```bash
$ fin

Source   Completed         Task
-------  ----------------  --------------------------------------
jira     2024-02-22 09:45  Update firewall config for app-123
jira     2024-02-22 09:28  Manage the micro-management of our middle-manager
todoist  2024-02-22 10:02  Walk cat
jira     2024-02-22 16:19  Review PR for core auth service
todoist  2024-02-23 13:33  Put on pants
```

### Flags

- `-d` - Number of days to go back. Default is 1.
  - Example: a value of 1 will return tasks from today and yesterday (1 day back).
- `-r` - Reverse the order of the list. Default is to display the most recently 
completed tasks last, like a chronological log.
- `-s` - Save the downloaded tasks in raw format to `testdata/`. This is useful for debugging and development.

### Environment variables

Env vars are used for authentication to Todoist and Jira.

#### [todoist.go](todoist/todoist.go)
- `TODOIST_TOKEN`

#### [jira.go](jira/jira.go)
- `JIRA_API_USER`
- `JIRA_API_TOKEN`
- `JIRA_HOST`
