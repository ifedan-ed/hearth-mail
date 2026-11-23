{{/* Names and labels shared by the templates. */}}
{{- define "hm.name" -}}{{ .Release.Name }}-hearth-mail{{- end -}}
{{- define "hm.labels" -}}
app.kubernetes.io/name: hearth-mail
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* The cluster DNS address Unbound forwards in-cluster names to. */}}
{{- define "hm.clusterDnsIp" -}}
{{- if .Values.unbound.clusterDnsIp -}}
{{ .Values.unbound.clusterDnsIp }}
{{- else -}}
{{- $svc := lookup "v1" "Service" "kube-system" "kube-dns" -}}
{{- if $svc -}}{{ $svc.spec.clusterIP }}{{- else -}}{{ fail "unbound.clusterDnsIp: set it (no kube-system/kube-dns Service to read it from)" }}{{- end -}}
{{- end -}}
{{- end -}}
