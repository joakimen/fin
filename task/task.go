package task

import (
	"fmt"
	"sort"
	"time"

	"github.com/joakimen/fin/config"
	"github.com/joakimen/fin/internal"
)

type (
	CollectorFunc func(cfg *config.Config) []Task
	Source        string
)

const (
	Todoist Source = "todoist"
	Jira    Source = "jira"
	GCal    Source = "gcal"
)

type Task struct {
	Source      Source
	StartsAt    time.Time
	CompletedAt time.Time
	Title       string
}

func FilterTasksWithinRequiredTime(tasks []Task, startDate time.Time) []Task {
	completedFiltered := make([]Task, 0, len(tasks))
	for _, task := range tasks {
		if task.CompletedAt.Before(startDate) || task.CompletedAt.Equal(startDate) {
			continue
		}
		completedFiltered = append(completedFiltered, task)
	}
	return completedFiltered
}

func SortByCompletedDate(tasks []Task, desc bool) {
	var sortFunc func(i, j int) bool
	if desc {
		sortFunc = func(i, j int) bool {
			return tasks[i].CompletedAt.After(tasks[j].CompletedAt)
		}
	} else {
		sortFunc = func(i, j int) bool {
			return tasks[i].CompletedAt.Before(tasks[j].CompletedAt)
		}
	}
	sort.Slice(tasks, sortFunc)
}

func PrintHeader() {
	fmt.Println()
	fmt.Println("Source   Completed         Task")
	fmt.Println("-------  ----------------  --------------------------------------")
}

func (t Task) String() string {
	completedDate := t.CompletedAt.Format("2006-01-02 15:04")
	taskTitle := internal.Ellipsis(t.Title, 80)
	return fmt.Sprintf("%-7s  %s  %-80s", t.Source, completedDate, taskTitle)
}
