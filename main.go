package main

import (
	"fmt"
	"github.com/joakimen/fin/gcal"
	"log/slog"

	"github.com/joakimen/fin/config"
	"github.com/joakimen/fin/task"
)

func main() {
	cfg := config.LoadConfig()
	taskCollectors := []task.CollectorFunc{
		//todoist.GetCompletedTasks,
		//jira.GetCompletedTasks,
		gcal.GetCompletedTasks,
	}
	taskCollectorCount := len(taskCollectors)
	tasksChan := make(chan []task.Task, taskCollectorCount)

	for _, taskCollector := range taskCollectors {
		go func(taskFunc task.CollectorFunc) {
			tasksChan <- taskFunc(cfg)
		}(taskCollector)
	}

	var allCompletedTasks []task.Task
	for range taskCollectorCount {
		tasks := <-tasksChan
		allCompletedTasks = append(allCompletedTasks, tasks...)
	}

	completedTasks := task.FilterTasksWithinRequiredTime(allCompletedTasks, cfg.StartDate)
	slog.Debug(fmt.Sprintf("excluded %d of %d completed tasks completed before start date",
		len(allCompletedTasks)-len(completedTasks), len(allCompletedTasks)))

	if len(completedTasks) > 0 {
		task.SortByCompletedDate(completedTasks, cfg.ReverseOutput)
		task.PrintHeader()
		for _, event := range completedTasks {
			fmt.Println(event)
		}
	} else {
		slog.Info("no completed tasks found since start date", slog.Time("start_date", cfg.StartDate))
	}
}
