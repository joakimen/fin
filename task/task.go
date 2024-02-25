package task

import (
	"fin/config"
	"fin/internal"
	"fmt"
	"sort"
	"time"
)

type CollectorFunc func(cfg *config.Config) []Task

type Source string

const (
	Todoist Source = "todoist"
	Jira    Source = "jira"
)

type Task struct {
	Source      Source
	CompletedAt time.Time
	Title       string
}

func FilterTasksCompletedWithinNDays(tasks []Task, days int) []Task {
	startDate := internal.GetStartOfDay(internal.GetDaysBack(days))
	var completedFiltered []Task
	for _, task := range tasks {
		if task.CompletedAt.After(startDate) {
			completedFiltered = append(completedFiltered, task)
		}
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
