package main

import (
	"fin/config"
	"fin/jira"
	"fin/task"
	"fin/todoist"
	"fmt"
)

func main() {
	cfg := config.LoadConfig()

	taskCollectors := []task.CollectorFunc{
		todoist.GetCompletedTasks,
		jira.GetCompletedTasks,
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

	if len(completedTasks) > 0 {
		task.SortByCompletedDate(completedTasks, cfg.ReverseOutput)
		task.PrintHeader()
		for _, event := range completedTasks {
			fmt.Println(event)
		}
	} else {
		fmt.Printf("no completed tasks found since start date %v.\n", cfg.StartDate)
	}
}
