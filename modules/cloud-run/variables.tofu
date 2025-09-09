variable "location" {
  description = "The location for the Cloud Run service"
  type        = string
}

variable "service_name" {
  description = "Name of the Cloud Run service"
  type        = string
}

variable "container_image" {
  description = "Container image URL"
  type        = string
}

variable "webhook_service_account_email" {
  description = "Service account email for the webhook service"
  type        = string
}

variable "vm_service_account_email" {
  description = "Service account email for the Actions VMs"
  type        = string
}

variable "vm_subnetwork_self_link" {
  description = "Subnetwork self-link for the Actions VMs"
  type        = string
}

variable "secrets" {
  description = "Map of environment variable names to Secret Manager secret names"
  type        = map(string)
}

variable "cpu_limit" {
  description = "CPU limit for Cloud Run service"
  type        = string
}

variable "memory_limit" {
  description = "Memory limit for Cloud Run service"
  type        = string
}

variable "cpu_request" {
  description = "CPU request for Cloud Run service"
  type        = string
}

variable "memory_request" {
  description = "Memory request for Cloud Run service"
  type        = string
}

variable "min_scale" {
  description = "Minimum number of instances"
  type        = number
}

variable "max_scale" {
  description = "Maximum number of instances"
  type        = number
}
