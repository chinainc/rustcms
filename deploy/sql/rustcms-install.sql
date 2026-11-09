-- MySQL dump 10.13  Distrib 5.7.38, for Win64 (x86_64)
--
-- Host: 127.0.0.1    Database: cms
-- ------------------------------------------------------
-- Server version	5.7.38-log

/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET @OLD_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS */;
/*!40101 SET @OLD_COLLATION_CONNECTION=@@COLLATION_CONNECTION */;
/*!40101 SET NAMES utf8mb4 */;
/*!40103 SET @OLD_TIME_ZONE=@@TIME_ZONE */;
/*!40103 SET TIME_ZONE='+00:00' */;
/*!40014 SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0 */;
/*!40014 SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0 */;
/*!40101 SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO' */;
/*!40111 SET @OLD_SQL_NOTES=@@SQL_NOTES, SQL_NOTES=0 */;

--
-- Table structure for table `admin_groups`
--

DROP TABLE IF EXISTS `admin_groups`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `admin_groups` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(32) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `setting` text,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `admin_groups`
--

LOCK TABLES `admin_groups` WRITE;
/*!40000 ALTER TABLE `admin_groups` DISABLE KEYS */;
INSERT INTO `admin_groups` (`id`, `created_time`, `updated_time`, `title`, `description`, `status`, `setting`) VALUES (1,1791860113,1791860113,'超级管理员','超级管理员分组',1,'{\"permissions\":null}');
/*!40000 ALTER TABLE `admin_groups` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `admin_login_logs`
--

DROP TABLE IF EXISTS `admin_login_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `admin_login_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `admin_id` int(10) unsigned NOT NULL DEFAULT '0',
  `ip` varchar(32) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `user_name` varchar(32) NOT NULL DEFAULT '',
  `password` varchar(128) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_admin_id` (`admin_id`)
) ENGINE=InnoDB AUTO_INCREMENT=125 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `admin_login_logs`
--

LOCK TABLES `admin_login_logs` WRITE;
/*!40000 ALTER TABLE `admin_login_logs` DISABLE KEYS */;
INSERT INTO `admin_login_logs` (`id`, `created_time`, `updated_time`, `admin_id`, `ip`, `status`, `user_name`, `password`) VALUES (1,1792642854,1792642854,1,'127.0.0.1',1,'admin',''),(2,1792643694,1792643694,1,'127.0.0.1',1,'admin',''),(3,1792643701,1792643701,1,'127.0.0.1',1,'admin',''),(4,1792643820,1792643820,1,'127.0.0.1',1,'admin',''),(5,1792643831,1792643831,1,'127.0.0.1',1,'admin',''),(6,1792643892,1792643892,1,'127.0.0.1',1,'admin',''),(7,1792648370,1792648370,1,'127.0.0.1',0,'admin','123456'),(8,1792648376,1792648376,1,'127.0.0.1',0,'admin','admin'),(9,1792648379,1792648379,1,'127.0.0.1',0,'admin','adminadmin'),(10,1792648395,1792648395,1,'127.0.0.1',0,'admin','123456'),(11,1792648399,1792648399,1,'127.0.0.1',0,'admin','abc123'),(12,1792648594,1792648594,1,'127.0.0.1',1,'admin',''),(13,1792648619,1792648619,1,'127.0.0.1',1,'admin',''),(14,1792648748,1792648748,1,'127.0.0.1',1,'admin',''),(15,1792648879,1792648879,1,'127.0.0.1',1,'admin',''),(16,1792648925,1792648925,1,'127.0.0.1',1,'admin',''),(17,1792648970,1792648970,1,'127.0.0.1',0,'admin','yinwang2025'),(18,1792648973,1792648973,1,'127.0.0.1',0,'admin','yinwang2026'),(19,1792648983,1792648983,1,'127.0.0.1',0,'admin','123456'),(20,1792648986,1792648986,1,'127.0.0.1',0,'admin','admin123456'),(21,1792649028,1792649028,1,'127.0.0.1',1,'admin',''),(22,1792649134,1792649134,1,'127.0.0.1',0,'admin','RustUpgrade@2026'),(23,1792649248,1792649248,1,'127.0.0.1',0,'admin','RustUpgrade@2026'),(24,1792649279,1792649279,1,'127.0.0.1',0,'admin','RustUpgrade@2026'),(25,1792649362,1792649362,1,'127.0.0.1',1,'admin',''),(26,1792649495,1792649495,1,'127.0.0.1',1,'admin',''),(27,1792649549,1792649549,1,'127.0.0.1',1,'admin',''),(28,1792650391,1792650391,1,'127.0.0.1',1,'admin',''),(29,1792650447,1792650447,1,'127.0.0.1',1,'admin',''),(30,1792650480,1792650480,1,'127.0.0.1',1,'admin',''),(31,1792651215,1792651215,1,'127.0.0.1',1,'admin',''),(32,1792651236,1792651236,1,'127.0.0.1',1,'admin',''),(33,1792651309,1792651309,1,'127.0.0.1',1,'admin',''),(34,1792653479,1792653479,1,'127.0.0.1',1,'admin',''),(35,1792654706,1792654706,1,'127.0.0.1',1,'admin',''),(36,1792654804,1792654804,1,'127.0.0.1',1,'admin',''),(37,1794043944,1794043944,1,'127.0.0.1',1,'admin',''),(38,1794043968,1794043968,1,'127.0.0.1',1,'admin',''),(39,1794043982,1794043982,1,'127.0.0.1',1,'admin',''),(40,1794043995,1794043995,1,'127.0.0.1',1,'admin',''),(41,1794044148,1794044148,1,'127.0.0.1',1,'admin',''),(42,1794044201,1794044201,1,'127.0.0.1',1,'admin',''),(43,1794044334,1794044334,1,'127.0.0.1',1,'admin',''),(44,1794044982,1794044982,1,'127.0.0.1',1,'admin',''),(45,1794044993,1794044993,1,'127.0.0.1',1,'admin',''),(46,1794045287,1794045287,1,'127.0.0.1',1,'admin',''),(47,1794045303,1794045303,1,'127.0.0.1',1,'admin',''),(48,1794045327,1794045327,1,'127.0.0.1',1,'admin',''),(49,1794045689,1794045689,1,'127.0.0.1',1,'admin',''),(50,1794045713,1794045713,1,'127.0.0.1',1,'admin',''),(51,1794045731,1794045731,1,'127.0.0.1',1,'admin',''),(52,1794045905,1794045905,1,'127.0.0.1',1,'admin',''),(53,1794045934,1794045934,1,'127.0.0.1',1,'admin',''),(54,1794046123,1794046123,1,'127.0.0.1',1,'admin',''),(55,1794046144,1794046144,1,'127.0.0.1',1,'admin',''),(56,1794046398,1794046398,1,'127.0.0.1',1,'admin','');
/*!40000 ALTER TABLE `admin_login_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `admin_logs`
--

DROP TABLE IF EXISTS `admin_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `admin_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `admin_id` int(10) unsigned NOT NULL DEFAULT '0',
  `ip` varchar(32) NOT NULL DEFAULT '',
  `log` varchar(250) NOT NULL DEFAULT '',
  `user_name` varchar(32) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_admin_id` (`admin_id`)
) ENGINE=InnoDB AUTO_INCREMENT=65 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `admin_logs`
--

LOCK TABLES `admin_logs` WRITE;
/*!40000 ALTER TABLE `admin_logs` DISABLE KEYS */;
INSERT INTO `admin_logs` (`id`, `created_time`, `updated_time`, `admin_id`, `ip`, `log`, `user_name`) VALUES (1,1792645722,1792645722,1,'127.0.0.1','上传升级包升级：3.6.8 -> 3.7.0（upgrade-test.zip，1 个文件，0.0 MB）','admin'),(2,1792645731,1792645731,1,'127.0.0.1','上传升级包升级：3.6.8 -> 3.7.0（upgrade-test.tar.gz，1 个文件，0.0 MB）','admin'),(3,1792648357,1792648357,1,'127.0.0.1','更新安全设置','admin'),(4,1792649024,1792649024,0,'127.0.0.1','重置管理员账号和密码：1 => admin',''),(5,1792649371,1792649371,1,'127.0.0.1','上传升级包升级：3.6.8 -> 3.7.0（upgrade-test.zip，1 个文件，0.0 MB）','admin'),(6,1792649504,1792649504,1,'127.0.0.1','上传升级包升级：3.6.8 -> 3.7.0（upgrade-test.zip，1 个文件，0.0 MB）','admin'),(7,1792650482,1792650482,1,'127.0.0.1','更新用户信息：1 => balance','admin'),(8,1792651216,1792651216,1,'127.0.0.1','审核会员认证 ','admin'),(9,1792651216,1792651216,1,'127.0.0.1','审核会员认证 ','admin'),(10,1792654719,1792654719,1,'127.0.0.1','保存可视化装修 home','admin'),(11,1792677828,1792677828,1,'127.0.0.1','更新文档：53 => 删除链路测试文档-20261022','admin'),(12,1792677840,1792677840,1,'127.0.0.1','删除文档：53 => 删除链路测试文档-20261022','admin'),(13,1792677852,1792677852,1,'127.0.0.1','删除文档：53 => 删除链路测试文档-20261022','admin'),(14,1792678189,1792678189,1,'127.0.0.1','更新文档：53 => 删除链路测试文档2-20261022','admin'),(15,1792678189,1792678189,1,'127.0.0.1','更新文档：54 => 删除链路测试文档3-20261022','admin'),(16,1792678201,1792678201,1,'127.0.0.1','删除文档：53 => 删除链路测试文档2-20261022','admin'),(17,1792678201,1792678201,1,'127.0.0.1','删除文档：53 => 删除链路测试文档2-20261022','admin'),(18,1792678201,1792678201,1,'127.0.0.1','删除文档：54 => 删除链路测试文档3-20261022','admin'),(19,1792678201,1792678201,1,'127.0.0.1','删除文档：54 => 删除链路测试文档3-20261022','admin'),(20,1792678217,1792678217,1,'127.0.0.1','修改文档模型：4 => 删除测试模型','admin'),(21,1792678222,1792678222,1,'127.0.0.1','保存文档分类：16 => 删除测试栏目','admin'),(22,1792678243,1792678243,1,'127.0.0.1','保存文档分类：16 => 删除测试栏目','admin'),(23,1792678249,1792678249,1,'127.0.0.1','更新文档：55 => 模型删除测试-已发布','admin'),(24,1792678250,1792678250,1,'127.0.0.1','更新文档：56 => 模型删除测试-草稿','admin'),(25,1792678341,1792678341,1,'127.0.0.1','删除文档模型：4 => 删除测试模型','admin'),(26,1792678356,1792678356,1,'127.0.0.1','更新文档：53 => 删除链路测试文档4-附件','admin'),(27,1792678357,1792678357,1,'127.0.0.1','删除文档：53 => 删除链路测试文档4-附件','admin'),(28,1792678357,1792678357,1,'127.0.0.1','删除文档：53 => 删除链路测试文档4-附件','admin'),(29,1792678363,1792678363,1,'127.0.0.1','更新文档：54 => 删除链路测试文档5-回收站','admin'),(30,1792678363,1792678363,1,'127.0.0.1','删除文档：54 => 删除链路测试文档5-回收站','admin'),(31,1792678363,1792678363,1,'127.0.0.1','恢复文档：54 => 删除链路测试文档5-回收站','admin'),(32,1792678370,1792678370,1,'127.0.0.1','删除文档：54 => 删除链路测试文档5-回收站','admin'),(33,1792678370,1792678370,1,'127.0.0.1','删除文档：54 => 删除链路测试文档5-回收站','admin');
/*!40000 ALTER TABLE `admin_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `admins`
--

DROP TABLE IF EXISTS `admins`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `admins` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_name` varchar(32) NOT NULL DEFAULT '',
  `password` varchar(128) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `login_time` bigint(20) DEFAULT '0',
  `group_id` int(10) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_user_name` (`user_name`)
) ENGINE=InnoDB AUTO_INCREMENT=990008 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `admins`
--

LOCK TABLES `admins` WRITE;
/*!40000 ALTER TABLE `admins` DISABLE KEYS */;
INSERT INTO `admins` (`id`, `created_time`, `updated_time`, `user_name`, `password`, `status`, `login_time`, `group_id`) VALUES (1,0,1794046398,'admin','$2b$12$vIH0dN45KYJ4M6cI5RUMu.FTBc0WaAw6zU936MOZLu9RutfrpCwqK',1,1794046398,1);
/*!40000 ALTER TABLE `admins` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_agent_checkpoint`
--

DROP TABLE IF EXISTS `ai_agent_checkpoint`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_agent_checkpoint` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `agent_id` int(10) unsigned NOT NULL COMMENT 'Agent ID',
  `session_id` varchar(64) NOT NULL DEFAULT '' COMMENT '会话ID',
  `round` int(10) NOT NULL DEFAULT '0' COMMENT '已完成轮数（下次从该轮继续）',
  `max_rounds` int(10) NOT NULL DEFAULT '0' COMMENT '本次执行允许的最大轮数',
  `status` tinyint(1) NOT NULL DEFAULT '1' COMMENT '1执行中 2完成 3失败',
  `created_at` bigint(20) DEFAULT NULL,
  `updated_at` bigint(20) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_agent_id` (`agent_id`),
  KEY `idx_status` (`status`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_agent_checkpoint`
--

LOCK TABLES `ai_agent_checkpoint` WRITE;
/*!40000 ALTER TABLE `ai_agent_checkpoint` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_agent_checkpoint` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_agent_logs`
--

DROP TABLE IF EXISTS `ai_agent_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_agent_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `agent_id` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '智能体ID',
  `session_id` varchar(64) NOT NULL DEFAULT '' COMMENT '本次执行的会话ID',
  `status` tinyint(1) NOT NULL DEFAULT '0' COMMENT '0执行中 1成功 2失败',
  `summary` text COMMENT '执行摘要',
  `error` text COMMENT '错误信息',
  `tool_calls` int(10) NOT NULL DEFAULT '0' COMMENT '工具调用次数',
  `started_at` bigint(20) NOT NULL DEFAULT '0' COMMENT '开始时间',
  `finished_at` bigint(20) NOT NULL DEFAULT '0' COMMENT '结束时间',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_agent_id` (`agent_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_agent_logs`
--

LOCK TABLES `ai_agent_logs` WRITE;
/*!40000 ALTER TABLE `ai_agent_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_agent_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_agents`
--

DROP TABLE IF EXISTS `ai_agents`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_agents` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `session_id` varchar(64) NOT NULL DEFAULT '' COMMENT '专属会话ID',
  `name` varchar(200) NOT NULL DEFAULT '' COMMENT '智能体名称',
  `strategy` text COMMENT '执行策略描述',
  `cron_expr` varchar(100) NOT NULL DEFAULT '' COMMENT 'Cron表达式',
  `enabled` tinyint(1) NOT NULL DEFAULT '1' COMMENT '1启用 0暂停',
  `last_run_at` bigint(20) NOT NULL DEFAULT '0' COMMENT '上次执行时间',
  `next_run_at` bigint(20) NOT NULL DEFAULT '0' COMMENT '下次执行时间',
  `run_count` int(10) NOT NULL DEFAULT '0' COMMENT '已执行次数',
  `max_runs` int(10) NOT NULL DEFAULT '0' COMMENT '最大执行次数(0=不限)',
  `max_rounds` int(10) NOT NULL DEFAULT '20' COMMENT '单次执行最大轮数(0=用默认20)',
  `last_summary` text COMMENT '上次执行摘要',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_session_id` (`session_id`),
  KEY `idx_enabled` (`enabled`),
  KEY `idx_next_run_at` (`next_run_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_agents`
--

LOCK TABLES `ai_agents` WRITE;
/*!40000 ALTER TABLE `ai_agents` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_agents` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_article_plans`
--

DROP TABLE IF EXISTS `ai_article_plans`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_article_plans` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `type` tinyint(1) NOT NULL DEFAULT '0',
  `req_id` bigint(20) NOT NULL DEFAULT '0',
  `language` varchar(10) NOT NULL DEFAULT '',
  `to_language` varchar(10) NOT NULL DEFAULT '',
  `keyword` varchar(250) NOT NULL DEFAULT '',
  `demand` text,
  `article_id` bigint(20) NOT NULL DEFAULT '0',
  `pay_count` int(10) NOT NULL DEFAULT '0',
  `use_self` tinyint(1) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_type` (`type`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_article_plans`
--

LOCK TABLES `ai_article_plans` WRITE;
/*!40000 ALTER TABLE `ai_article_plans` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_article_plans` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_chat_messages`
--

DROP TABLE IF EXISTS `ai_chat_messages`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_chat_messages` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `seq` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '消息序号',
  `session_id` varchar(64) NOT NULL DEFAULT '',
  `role` varchar(16) NOT NULL DEFAULT '',
  `content` longtext,
  `tool_call_id` varchar(64) NOT NULL DEFAULT '' COMMENT '工具调用ID',
  `tool_name` varchar(64) NOT NULL DEFAULT '' COMMENT '工具名称',
  `tool_calls` longtext COMMENT '工具调用 JSON',
  `turn_id` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '回合ID',
  `is_summary` tinyint(1) NOT NULL DEFAULT '0' COMMENT '是否摘要消息',
  `summary_of` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '摘要覆盖起始ID',
  `files` text COMMENT '上传文件列表 JSON',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_seq` (`seq`),
  KEY `idx_session_id` (`session_id`),
  KEY `idx_ai_chat_messages_tool_call_id` (`tool_call_id`),
  KEY `idx_turn_id` (`turn_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_chat_messages`
--

LOCK TABLES `ai_chat_messages` WRITE;
/*!40000 ALTER TABLE `ai_chat_messages` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_chat_messages` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_mcp_audit_logs`
--

DROP TABLE IF EXISTS `ai_mcp_audit_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_mcp_audit_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `site_id` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '目标站点ID',
  `token_mask` varchar(64) NOT NULL DEFAULT '' COMMENT 'Token掩码',
  `tool` varchar(64) NOT NULL DEFAULT '' COMMENT '工具名',
  `risk` varchar(16) NOT NULL DEFAULT 'read' COMMENT '风险等级',
  `args_digest` varchar(64) NOT NULL DEFAULT '' COMMENT '参数SHA256',
  `args_preview` text COMMENT '参数前500字符预览',
  `ok` tinyint(1) NOT NULL DEFAULT '1' COMMENT '1成功 0失败',
  `err_msg` text COMMENT '错误信息',
  `cost_ms` bigint(20) NOT NULL DEFAULT '0' COMMENT '耗时毫秒',
  `client_ip` varchar(64) NOT NULL DEFAULT '' COMMENT '调用方IP',
  `user_agent` varchar(255) NOT NULL DEFAULT '' COMMENT 'User-Agent',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_site_id` (`site_id`),
  KEY `idx_tool` (`tool`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_mcp_audit_logs`
--

LOCK TABLES `ai_mcp_audit_logs` WRITE;
/*!40000 ALTER TABLE `ai_mcp_audit_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_mcp_audit_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `ai_usage_logs`
--

DROP TABLE IF EXISTS `ai_usage_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `ai_usage_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `session_id` varchar(64) NOT NULL DEFAULT '' COMMENT '会话ID',
  `agent_id` int(10) unsigned NOT NULL DEFAULT '0' COMMENT '智能体ID (0=主会话)',
  `turn_id` int(10) NOT NULL DEFAULT '0' COMMENT '本轮轮次编号',
  `model_name` varchar(128) NOT NULL DEFAULT '' COMMENT '模型名称',
  `prompt_tokens` int(10) NOT NULL DEFAULT '0' COMMENT '输入token',
  `completion_tokens` int(10) NOT NULL DEFAULT '0' COMMENT '输出token',
  `total_tokens` int(10) NOT NULL DEFAULT '0' COMMENT '总token',
  `cost` decimal(12,6) NOT NULL DEFAULT '0.000000' COMMENT '成本(USD)',
  `tool_calls` int(10) NOT NULL DEFAULT '0' COMMENT '本轮工具调用次数',
  `duration_ms` bigint(20) NOT NULL DEFAULT '0' COMMENT '本轮耗时(毫秒)',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_session_id` (`session_id`),
  KEY `idx_agent_id` (`agent_id`),
  KEY `idx_model_name` (`model_name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `ai_usage_logs`
--

LOCK TABLES `ai_usage_logs` WRITE;
/*!40000 ALTER TABLE `ai_usage_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `ai_usage_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `anchor_data`
--

DROP TABLE IF EXISTS `anchor_data`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `anchor_data` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `anchor_id` bigint(20) NOT NULL DEFAULT '0',
  `item_type` varchar(32) NOT NULL DEFAULT '',
  `item_id` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_anchor_data_anchor_id` (`anchor_id`),
  KEY `idx_item_type` (`item_type`,`item_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `anchor_data`
--

LOCK TABLES `anchor_data` WRITE;
/*!40000 ALTER TABLE `anchor_data` DISABLE KEYS */;
/*!40000 ALTER TABLE `anchor_data` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `anchors`
--

DROP TABLE IF EXISTS `anchors`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `anchors` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(190) NOT NULL DEFAULT '',
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `link` varchar(190) NOT NULL DEFAULT '',
  `weight` int(10) NOT NULL DEFAULT '0',
  `replace_count` int(10) NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_anchors_title` (`title`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_anchors_link` (`link`),
  KEY `idx_weight` (`weight`),
  KEY `idx_status` (`status`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `anchors`
--

LOCK TABLES `anchors` WRITE;
/*!40000 ALTER TABLE `anchors` DISABLE KEYS */;
/*!40000 ALTER TABLE `anchors` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_categories`
--

DROP TABLE IF EXISTS `archive_categories`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_categories` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_category_id` (`category_id`),
  KEY `idx_archive_category_id` (`archive_id`,`category_id`)
) ENGINE=InnoDB AUTO_INCREMENT=19 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_categories`
--

LOCK TABLES `archive_categories` WRITE;
/*!40000 ALTER TABLE `archive_categories` DISABLE KEYS */;
INSERT INTO `archive_categories` (`id`, `category_id`, `archive_id`) VALUES (1,13,49),(2,13,50),(3,3,51),(15,3,52),(16,4,54),(17,4,55);
/*!40000 ALTER TABLE `archive_categories` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_data`
--

DROP TABLE IF EXISTS `archive_data`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_data` (
  `id` bigint(20) unsigned NOT NULL AUTO_INCREMENT,
  `content` longtext,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=77 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_data`
--

LOCK TABLES `archive_data` WRITE;
/*!40000 ALTER TABLE `archive_data` DISABLE KEYS */;
INSERT INTO `archive_data` (`id`, `content`) VALUES (1,'<p>RustCMS 2026 春季版本日前正式发布。本次版本的重点是工作流引擎的全面升级，新增可视化流程设计器、条件分支、并行网关和跨系统回调能力，复杂审批场景的配置时间大幅缩短。</p><p>新引擎采用事件驱动架构，流程实例与业务数据解耦，支持流程版本热切换和历史实例追溯。在内部压测中，单节点每小时可处理超过 20 万次流程流转，审批平均耗时下降约 40%。</p><p>此外，本次版本还优化了移动端体验，新增消息聚合和待办提醒，并开放了流程节点插件接口，便于合作伙伴扩展行业模板。</p>'),(2,'<p>近日，省工业和信息化厅公布新一批“专精特新”中小企业名单，公司凭借在企业级软件领域的持续投入和产品创新成功入选。</p><p>近年来，公司保持较高的研发投入强度，围绕低代码引擎、数据集成和工作流内核形成了多项自主知识产权，产品和解决方案已服务数百家企业客户。</p><p>公司负责人表示，获评“专精特新”既是认可也是鞭策，公司将继续聚焦主业，把产品做深做透，为更多企业的数字化转型提供稳定可靠的软件底座。</p>'),(3,'<p>RustCMS 低代码平台 3.0 正式发布。新版本重构了表单引擎和数据模型设计器，支持复杂业务规则、动态权限和多端发布，开发者可以通过拖拽完成大部分页面搭建。</p><p>平台内置了流程、报表、消息和集成能力，业务人员可以在IT人员的配合下快速搭建应用。在实际项目中，标准业务的开发周期由原来的两周缩短到一周以内，整体交付效率提升约 50%。</p><p>3.0 版本同时开放了组件市场和插件机制，合作伙伴可以将行业组件沉淀为可复用资产，进一步降低交付成本。</p>'),(4,'<p>公司与国内某云厂商签署生态合作协议，双方将在产品适配、联合方案和区域市场拓展等方面展开深入合作。</p><p>根据协议，RustCMS 产品将完成在该云平台上的兼容性认证和性能优化，双方联合推出面向制造行业的云上 ERP 与数据中台方案，为客户提供从基础设施到应用软件的一体化选择。</p><p>公司负责人表示，云厂商在基础设施和区域渠道上具有优势，RustCMS 在企业管理软件领域积累深厚，双方能力互补，有望为客户带来更低的总体拥有成本。</p>'),(5,'<p>公司与本地高校计算机学院共建的“软件工程联合实验室”正式揭牌。双方将围绕低代码引擎、软件质量保障和智能编程辅助等方向开展联合研究。</p><p>实验室将采用“企业出题、高校解题、成果落地”的模式，企业提供真实业务场景和数据，高校团队负责前沿技术研究，研究成果优先在公司产品中验证和应用。</p><p>同时，双方还将联合开展研究生培养、实习实训和课程共建，为软件行业输送更多既懂理论又懂工程实践的人才。</p>'),(6,'<p>2026 中国国际软件博览会日前举行，公司携 ERP、低代码平台和数据中台三条产品线参展，集中展示企业数字化转型的整体能力。</p><p>展台现场，观众可以通过互动大屏体验从业务建模、流程配置到报表分析的完整流程。三天展期共接待专业观众数百人次，与多家制造、零售企业达成初步合作意向。</p><p>公司还参与了大会的数字化转型分论坛，分享了中小制造企业低成本数字化的实践经验。</p>'),(7,'<p>经过数月的准备与现场评估，公司正式通过 CMMI 3 级评估。这标志着公司在需求管理、项目计划、开发实施、测试验证和过程改进等方面建立了标准化、可复制的管理体系。</p><p>评估过程中，评估师抽查了多个在建项目的过程文档和实际执行情况，对公司的项目管理和质量保障能力给予肯定，同时也提出了持续改进建议。</p><p>公司将以 CMMI 3 级为新起点，把过程要求落到日常研发中，在保证交付质量的同时提升响应速度。</p>'),(8,'<p>公司近日通过 ISO/IEC 27001 信息安全管理体系认证，认证范围覆盖软件研发、项目实施和运维服务全过程。</p><p>在体系建设过程中，公司梳理了信息资产清单，完善了访问控制、数据备份、漏洞管理和应急响应等制度，并对全体员工开展了信息安全培训。</p><p>公司表示，信息安全是软件服务商的底线。未来将持续开展风险评估和内部审计，把安全要求嵌入研发和交付流程，为客户数据提供体系化保障。</p>'),(9,'<p>近日，省科学技术奖励大会召开，公司参与完成的“面向企业应用的低代码开发关键技术及应用”项目获得省科技进步三等奖。</p><p>该项目围绕可视化建模、元数据驱动和动态表单等关键技术展开研究，形成了可复用的低代码引擎，并在多个行业项目中得到验证，显著降低了企业应用开发的门槛和成本。</p><p>公司将继续加大在低代码、数据智能方向的研发投入，推动科研成果在更多业务场景中落地。</p>'),(10,'<p>公司正式启动“工程师成长计划”，面向研发、实施和运维团队建立分层分类的培养体系。</p><p>计划包括三个部分：一是导师制，由资深工程师一对一辅导新员工；二是技术分享，每月组织内部技术沙龙和专题培训；三是认证激励，鼓励员工参加云计算、数据库和项目管理等专业认证，并给予费用支持。</p><p>公司希望借此形成持续学习的组织氛围，让工程师在技术深度和业务理解上同步成长，为客户提供更专业的服务。</p>'),(11,'<p>1024 程序员节当天，公司举办了一场技术嘉年华，近两百名研发、实施和产品同事参与。</p><p>活动设置了算法擂台、代码审查挑战和开源项目分享等环节，多个团队展示了在性能优化、自动化测试和低代码组件方面的实践成果。下午的团队拓展则让大家在轻松的氛围中加深了解。</p><p>公司负责人表示，工程师是公司最宝贵的资产，希望通过这样的活动让大家在交流中互相启发，也感受到团队的活力。</p>'),(12,'<p>客户是拥有多个生产基地的汽车零部件集团，原有系统版本老旧、各基地数据独立，集团难以及时掌握经营情况。项目组为其制定了分阶段 ERP 升级方案。</p><p>一期完成财务、采购、库存和销售的统一，建立集团主数据和科目体系；二期扩展到生产与成本核算，实现财务业务一体化。升级过程中采用新旧系统并行和数据校验，确保业务不中断。</p><p>上线后，集团月结时间由 10 天缩短到 4 天，库存周转率提升 18%，管理层可以通过统一看板实时查看各基地经营数据。</p>'),(13,'<p>客户为航空零部件制造企业，产品批次多、工艺复杂，质量追溯要求高。项目组围绕工单、批次和工序搭建 MES。</p><p>系统实现从原材料入库到成品交付的全过程记录，每个关键工序采集设备、人员、参数和检验数据，形成完整的质量档案。异常情况触发隔离和评审流程，防止不合格品流转。</p><p>实施后，质量追溯时间由原来的数天缩短到几分钟，生产计划达成率提升 20%，为客户通过主机厂审核提供了有力支撑。</p>'),(14,'<p>医疗器械行业对质量追溯有严格监管要求。客户希望打通原料、生产、检验、流通和售后环节，实现全链路追溯。</p><p>项目组以产品唯一标识为主线，建立批次和序列号管理，记录关键工序、检验结果和放行信息，并与经销和售后数据关联。系统支持召回演练，可快速定位受影响产品范围。</p><p>系统上线后顺利通过监管检查，召回准备时间由数天缩短到数小时，客户质量管理的规范化水平明显提升。</p>'),(15,'<p>客户是家电集团，线上商城、线下门店和多个电商平台各自运营，会员权益和库存不统一，促销活动难以协同。</p><p>项目组建设全渠道中台，统一商品和会员体系，打通订单与库存。消费者可以线上下单门店自提，也可以门店开单线上发货。会员积分和优惠券在全渠道通用。</p><p>项目上线后，会员复购率提升 22%，跨渠道订单占比达到 35%，库存共享减少了重复备货。</p>'),(16,'<p>客户是大型建筑集团，下设多家分子公司和数百个项目部，原有审批靠纸质和邮件，流程跟踪困难。</p><p>项目组搭建协同办公平台，覆盖集团、分子公司和项目部三级组织，统一公文、审批、用印、请假和费用流程。移动端支持项目现场发起申请和拍照上传，审批进度实时可见。</p><p>平台上线后，平均审批时长缩短 60%，项目部与机关的沟通效率明显提升，流程数据也为管理审计提供了依据。</p>'),(17,'<p>制造企业的数字化转型，核心是让业务过程在线、让数据可信、让决策有依据。我们以 ERP 为经营管理底座，以 MES 为生产执行抓手，以数据中台为分析支撑，形成覆盖计划、生产、供应链和财务的完整方案。</p><p>方案通常分三步实施：第一步统一主数据和基础编码，打通订单到交付的主流程；第二步上线 MES 和质量管理，实现生产过程透明；第三步建设数据中台，沉淀指标体系和经营看板。</p><p>我们提供从调研、蓝图设计到系统实施、培训和运维的全周期服务，并根据企业规模提供标准化或定制化组合，控制投入风险。</p>'),(18,'<p>客户服务正在从“人海战术”转向“知识驱动”。我们的智能客服方案由知识库、对话引擎和工单系统三部分组成，支持网页、公众号、小程序等多渠道接入。</p><p>知识库负责统一沉淀产品资料、常见问题和处理规范；对话引擎基于大模型和检索增强技术，理解客户问题并给出有依据的回答；遇到复杂问题时可一键转人工，并自动带出历史会话和客户信息。</p><p>系统还会对会话进行质检和聚类分析，帮助客户发现高频问题和产品改进点，形成服务闭环。</p>'),(19,'<p>能源行业设备数量多、数据频率高，传统人工抄表和分散系统难以支撑精细化管理。我们提供从边缘采集到云端分析的一体化方案。</p><p>方案通过边缘网关接入 PLC、电表和传感器数据，完成协议解析和本地缓存；云端采用时序数据库存储海量测点，支持实时监控、告警和 histor 数据查询。在此基础上构建能耗分析和设备健康度模型，辅助运维决策。</p><p>系统支持多站点统一管理和权限隔离，可根据企业规模灵活部署在私有云或公有云环境。</p>'),(20,'<p>供应链的痛点往往不在单个环节，而在信息不对称：采购看不到库存，供应商不知道交期，财务对账靠人工。我们的供应链协同方案以订单为主线，连接企业、供应商和仓库。</p><p>方案提供采购申请、询价比价、订单协同、送货预约、质检入库和自动对账等能力，供应商可以通过门户确认交期、上传发货信息，库存数据实时同步。</p><p>系统内置供应商绩效分析，从交付及时率、质量合格率和价格波动等维度进行评估，帮助企业优化供应结构。</p>'),(21,'<p>系统上线只是开始，能不能用好取决于人。我们提供面向企业管理者、业务骨干和 IT 人员的数字化人才培养方案。</p><p>方案包括三部分：一是分层课程，覆盖数字化认知、流程管理、数据分析和系统操作；二是场景实训，基于客户的真实业务数据在沙箱环境中练习；三是认证考核，通过考试和项目实践评估学习效果。</p><p>培训可与实施项目结合，在系统上线前完成关键用户培养，缩短磨合期，降低后续运维压力。</p>'),(22,'<p>高可用不是某一个组件的能力，而是整条链路的设计结果。在云原生架构下，我们通常从副本、健康检查和流量治理三个层面入手。</p><p>首先，无状态服务至少部署两个副本，并通过健康检查自动摘除异常实例；数据库采用主从或集群模式，关键数据定期备份并验证可恢复性。其次，引入熔断、限流和重试策略，避免单点故障扩散。最后，跨可用区部署和定期故障演练，确保真实故障时系统能够自动切换。</p><p>高可用建设需要持续投入，建议从核心链路开始，逐步覆盖全部业务。</p>'),(23,'<p>微服务不是越多越好，拆分前应先回答五个关键问题。</p><p>第一是边界：按业务能力还是按数据归属划分，建议以领域驱动设计为基础，识别限界上下文。第二是数据：每个服务是否独立持有数据，跨服务查询如何处理。第三是通信：同步调用还是事件驱动，需要明确失败和重试策略。第四是部署：服务是否能够独立发布，依赖关系是否清晰。第五是治理：日志、链路追踪、配置和权限如何统一。</p><p>如果团队规模和组织能力尚不匹配，优先考虑模块化单体，等情况成熟再逐步拆分。</p>'),(24,'<p>前端性能直接影响用户体验和转化率。以下十项技巧在项目中落地成本较低、收益明显。</p><p>资源方面：压缩图片并使用合适格式，按路由拆分代码，移除未使用的依赖，开启 gzip 或 brotli。加载方面：关键资源预加载，非关键脚本延迟加载，首屏数据服务端渲染。缓存方面：为静态资源配置长效缓存和内容指纹，接口数据按业务特性设置缓存策略。渲染方面：减少布局抖动，避免长任务阻塞主线程，列表采用虚拟滚动。</p><p>优化前先用性能面板建立基线，优化后用真实用户监控验证效果，避免凭感觉调优。</p>'),(25,'<p>很多数据中台项目失败，不是技术不行，而是治理缺位。数据治理需要从标准、质量、安全和运营四个方面持续投入。</p><p>标准方面，统一主数据、业务编码和指标口径，形成可维护的指标字典。质量方面，建立完整性、唯一性、及时性等校验规则，对异常数据自动告警并跟踪处理。安全方面，按数据分级分类设置访问权限，敏感数据脱敏使用。运营方面，明确数据责任人和考核机制，定期评估数据使用情况。</p><p>治理不是一次性项目，建议设立数据治理委员会，把治理要求嵌入日常业务流程。</p>'),(26,'<p>AI 辅助编程已经在研发团队中普及，但要真正提升效率，需要解决场景选择和质量控制两个问题。</p><p>场景方面，优先用于样板代码生成、单元测试编写、代码解释和文档补全等低风险任务；核心业务逻辑和涉及安全的代码仍需人工主导。质量方面，建立使用规范，要求开发者对 AI 生成的代码进行审查和测试，禁止直接提交未经理解的代码。</p><p>团队还应关注代码版权和敏感信息保护，避免将客户数据或密钥发送到外部服务。通过试点和复盘，逐步形成适合团队的使用手册。</p>'),(27,'<p>我们提供企业官网、内部信息门户和供应商门户的定制开发服务，涵盖需求梳理、视觉设计、前端开发、系统集成和上线运维。</p><p>官网注重品牌表达和访问性能，内部门户强调信息聚合和单点登录，供应商门户则聚焦订单协同和资料交互。三类门户均可与 ERP、OA 等系统对接，实现数据同步。</p><p>项目采用响应式设计，适配 PC、平板和手机，并配套内容管理后台，方便企业自行维护。</p>'),(28,'<p>我们提供微信小程序、企业微信应用和 H5 页面的定制开发，覆盖商城、会员、预约、问卷、活动报名和内部工具等场景。</p><p>开发过程包括原型设计、UI 设计、接口开发、联调测试和上线发布。对于需要与现有系统打通的场景，我们会提供统一的接口方案和权限设计。</p><p>上线后提供版本迭代和运维支持，帮助企业持续运营移动端用户。</p>'),(29,'<p>企业往往同时运行多套系统，数据重复录入和口径不一致是常见问题。我们提供系统集成与接口开发服务，帮助客户打通业务链路。</p><p>服务内容包括接口梳理、数据映射、API 开发、消息队列集成和定时同步。对于老旧系统，可通过数据库视图、文件交换或中间表等方式实现过渡。</p><p>我们还会建立接口清单和监控机制，记录调用量、成功率和异常情况，避免集成关系失控。</p>'),(30,'<p>运行多年的老系统常常面临技术栈过时、文档缺失和维护人员流失的问题。我们提供遗留系统的评估、迁移和重构服务。</p><p>项目通常从资产盘点开始，梳理业务功能、数据结构和外部依赖，评估迁移风险和优先级。根据评估结果，可选择整体重构、逐步替换或封装集成的策略。</p><p>迁移过程中会建立数据校验和回滚方案，确保业务连续性。重构后的系统采用现代化架构，便于后续扩展和运维。</p>'),(31,'<p>RustBI 是面向业务人员的商业智能平台，提供数据接入、指标建模、可视化分析和自助报表能力。</p><p>平台内置丰富的图表组件和拖拽式分析界面，业务人员无需编写代码即可完成多维分析和钻取。支持移动端查看和定时订阅，让数据主动触达管理者。</p><p>通过与数据中台和业务系统集成，RustBI 可以快速构建经营分析、销售分析和生产分析等主题看板。</p>'),(32,'<p>RustReport 是一款企业级报表设计器，支持类 Excel 的报表布局、分组汇总、交叉表、图表和套打，能够满足财务、生产和销售等场景的复杂报表需求。</p><p>报表数据可来自数据库、接口或文件，设计完成后可定时生成并推送到邮箱和移动端。支持 PDF、Excel、Word 和图片等多种导出格式。</p><p>设计器提供权限控制和数据脱敏，确保不同角色只能查看授权范围内的数据。</p>'),(33,'<p>RustData 数据集成平台支持数据库、接口、文件和消息队列等多种数据源，提供批量抽取、实时同步和增量更新能力。</p><p>平台内置任务调度、依赖编排、失败重试和运行监控，运维人员可以直观看到每个任务的执行状态和数据量。数据质量模块支持完整性、唯一性和及时性校验，异常自动告警。</p><p>通过统一的数据接入规范，企业可以逐步消除信息孤岛，为数据中台和分析应用打好基础。</p>'),(34,'<p>RustMetric 帮助企业建立统一的指标管理体系，覆盖指标定义、计算口径、数据来源、责任人和使用范围。</p><p>平台支持指标分层分类和血缘分析，业务人员可以查看每个指标的计算逻辑和上下游依赖。指标变更需经过审批，历史版本可追溯，避免同名不同义。</p><p>通过与 BI 和数据中台集成，指标可以自动生成分析看板，确保全公司使用同一套数据语言。</p>'),(35,'<p>RustOA 是面向组织的协同办公平台，提供统一门户、流程审批、公文管理、会议安排、任务协作和知识库等能力，让日常办公在一个入口完成。</p><p>平台支持多组织、多层级权限，流程可按部门、角色和条件灵活配置。移动端与 PC 端体验一致，待办消息可推送到企业微信和钉钉。</p><p>通过开放接口，RustOA 可以与 ERP、HR 和第三方业务系统集成，避免形成新的信息孤岛。</p>'),(36,'<p>RustFlow 是可视化工作流引擎，支持拖拽式流程设计，提供条件分支、并行网关、会签、加签、转办和超时提醒等常见审批能力。</p><p>引擎采用事件驱动架构，流程定义与业务数据解耦，支持流程版本热切换和历史实例追溯。通过开放 API 和回调机制，可以嵌入 ERP、OA 等业务系统，也可以独立部署使用。</p><p>内置流程监控和效率分析，帮助管理者发现流程瓶颈，持续优化审批路径。</p>'),(37,'<p>RustDocs 帮助企业集中管理合同、制度、方案和技术文档，支持在线预览、全文检索、版本控制和细粒度权限。</p><p>文档可按组织、项目和标签分类，支持多人协同编辑和审批发布。系统记录每次修改和下载行为，满足审计要求。</p><p>通过知识库和问答能力，员工可以快速找到所需资料，减少重复沟通。</p>'),(38,'<p>RustMeet 是面向企业的视频会议系统，支持高清音视频、屏幕共享、互动白板和会议录制，可通过浏览器或客户端加入。</p><p>系统与日程和审批联动，会议预约后自动通知参会人，会议纪要可一键归档到文档中心。支持会议室终端和移动端接入，满足日常例会、客户沟通和远程培训场景。</p><p>私有化部署方案可将音视频服务部署在企业内网，保障数据安全。</p>'),(39,'<p>制造企业的车间管理常常依赖纸质单据和人工统计，生产进度不透明、质量追溯困难。我们的 MES 方案以工单为主线，连接计划、车间和设备。</p><p>方案覆盖计划排产、工序报工、物料防错、质量检验、设备点检和异常呼叫等场景，通过看板实时展示生产进度和瓶颈工序。每件产品都可追溯到工单、批次、设备和操作人员。</p><p>系统可与 ERP、WMS 和设备采集平台集成，形成计划到执行再到分析的闭环。</p>'),(40,'<p>零售企业往往同时经营线上商城、门店和第三方平台，订单、会员和库存分散在不同系统中，难以统一运营。全渠道中台方案帮助客户整合这些能力。</p><p>方案提供统一的商品中心、会员中心、订单中心和库存中心，支持线上下单门店自提、门店发货和跨渠道退换货。会员积分和优惠券在全渠道通用，营销活动可统一配置。</p><p>通过数据分析，运营人员可以查看渠道贡献、会员复购和商品动销情况，及时调整策略。</p>'),(41,'<p>医疗行业的信息化建设既要提升患者体验，也要满足数据安全和合规要求。我们提供患者服务、运营管理和数据治理三类方案。</p><p>患者服务方面，支持预约挂号、在线问诊、报告查询和满意度调查；运营管理方面，覆盖排班、物资、设备和绩效；数据治理方面，建立患者主索引和数据访问审计，敏感信息脱敏使用。</p><p>方案支持私有化部署和等保要求，可与医院现有 HIS 系统对接，避免重复建设。</p>'),(42,'<p>智慧校园平台面向中小学和职业院校，覆盖教务管理、学生管理、教师发展、家校协同和校园服务等场景。</p><p>平台提供选课排课、成绩分析、德育评价、请假审批和家校沟通等能力，教师和家长可以通过移动端及时获取信息。校园服务包括报修、场地预约和访客管理。</p><p>所有数据按角色和班级隔离，操作留痕，满足教育主管部门的监管要求。</p>'),(43,'<p>RustMES 是以工单为核心的车间执行系统，覆盖计划下达、工序派工、扫码报工、质量检验和产品追溯等环节。</p><p>操作人员通过工位终端或移动设备扫码报工，系统自动记录工时、产量和不良品；质检环节按检验规范采集数据，异常自动触发返工或隔离流程。</p><p>车间看板实时展示计划完成率、设备状态和在制品情况，帮助管理者快速发现瓶颈。</p>'),(44,'<p>RustWMS 面向制造和流通企业的仓储管理，支持多仓库、多货主、多批次和条码管理。</p><p>系统覆盖收货、质检、上架、移库、拣货、复核、出库和盘点全流程，通过策略配置实现先进先出和库位优化。手持终端扫码作业，减少人工录入错误。</p><p>库存数据实时同步到 ERP 和 MES，支持安全库存预警和呆滞料分析，帮助企业降低库存资金占用。</p>'),(45,'<p>RustQMS 帮助企业建立覆盖来料检验、过程检验、成品检验和售后质量反馈的完整体系。</p><p>系统支持检验标准、抽样方案和检验记录管理，不合格品可发起评审、返工或让步接收流程。每批物料和产品都记录批次、供应商和检验结果，支持正向和反向追溯。</p><p>质量看板展示合格率、不良分布和改进趋势，为质量改进提供数据依据。</p>'),(46,'<p>RustCRM 帮助企业管理客户档案、联系人、商机、合同和售后服务，形成完整的客户视图。</p><p>销售可以通过移动端记录拜访、创建商机和设置跟进提醒，销售漏斗直观展示各阶段商机金额和转化率。合同与回款信息联动，逾期自动提醒。</p><p>服务模块支持工单受理、派工和处理回访，客户历史沟通记录集中呈现，提升服务响应质量。</p>'),(47,'<p>RustHR 覆盖组织架构、员工档案、考勤排班、招聘入职、合同管理和员工自助等核心场景。</p><p>系统支持多组织、多考勤方案和复杂排班，考勤数据可自动汇总并与薪酬联动。招聘模块管理职位、简历和面试流程，入职后自动建立员工档案。</p><p>员工可以通过自助端查看薪资、申请休假和更新个人信息，减少人事事务性工作。</p>'),(48,'<p>RustFin 面向集团企业建设财务共享中心，统一各分子公司的核算、对账和报表流程。</p><p>平台支持单据影像化、任务池分配和作业时效管理，费用、应付和总账业务按标准流程集中处理。银企直联和自动对账减少手工操作，月结效率明显提升。</p><p>集团管理层可以实时查看各单位的财务状况和共享中心作业量，为资源调配提供依据。</p>'),(56,'<p>近日，公司顺利通过 ISO 9001:2015 质量管理体系年度监督审核。审核组围绕需求评审、设计开发、测试验证、项目交付和运维服务等环节进行了现场检查。</p><p>过去一年，公司完善了需求变更管理和缺陷闭环机制，关键项目的按期交付率提升至 96%，客户投诉平均响应时间缩短到 2 小时以内。</p><p>公司将继续以体系文件为基础，把质量要求落实到每个项目和每个岗位，为客户提供更稳定可靠的软件与服务。</p>'),(57,'<p>RustCMS 自主研发的 RustData 数据中台完成出厂前的稳定性压测，并正式交付首批客户。</p><p>压测在亿级数据量、数百个并发任务条件下进行，重点验证了数据同步、任务调度和查询服务的稳定性。测试结果显示，核心接口平均响应时间保持在 200 毫秒以内，连续运行 72 小时无故障。</p><p>交付后，公司实施团队将协助客户完成数据源接入、指标体系梳理和报表搭建，并在首月提供驻场支持，确保平台平稳运行。</p>'),(58,'<p>2026 数字中国建设峰会日前开幕，公司携数据中台和低代码平台参展，展示在政务数据治理和企业数字化转型中的应用成果。</p><p>展会现场，公司演示了从多源数据接入、指标建模到可视化分析的全流程，并介绍了在制造企业供应链协同、园区数据治理等场景的落地案例。</p><p>峰会期间，公司与多家单位就数据共享、系统集成等议题进行了交流，达成多项合作意向。</p>'),(59,'<p>RustERP 是面向中型企业的集成化管理平台，覆盖财务核算、采购供应、库存、生产、销售和人力资源等核心业务，帮助企业建立统一的主数据、流程和报表体系。</p><p>平台采用模块化设计，企业可以按需启用，降低一次性投入。内置的多组织、多账套和多币种能力，适合集团型企业和跨区域经营场景。</p><p>系统提供开放接口和低代码扩展能力，便于与 MES、CRM 和第三方系统集成。</p>'),(60,'<p>RustERP 财务核算模块覆盖总账、应收应付、固定资产、资金和财务报表等核心能力，与采购、销售、库存等业务单据实时联动，减少手工凭证和重复录入。</p><p>模块支持多账套、多币种和多期间管理，提供凭证模板、自动记账和期末结账检查，帮助财务人员把精力从核算转向分析。</p><p>内置的财务报表和经营分析看板可按组织、产品和客户维度展开，满足管理层对经营情况的实时掌握。</p>'),(61,'<p>RustOA 移动办公套件提供 APP 和微信小程序两种入口，员工可以随时随地处理待办、发起申请、查看公告和联系同事。</p><p>套件支持移动审批、考勤打卡、外勤签到、通讯录和消息推送，并与 PC 端数据实时同步。对于不方便安装 APP 的场景，小程序可以快速接入。</p><p>移动端采用统一的安全策略，支持设备绑定、水印和远程注销，降低移动办公的数据风险。</p>'),(62,'<p>客户是汽车零部件制造企业，计划、生产、质量和设备数据分散，车间管理依赖人工统计。</p><p>项目以 ERP 和 MES 为核心，打通订单、工单、报工、质检和设备采集。车间部署电子看板，实时展示计划完成率、设备状态和异常呼叫。质量数据按批次归档，支持快速追溯。</p><p>项目投产后，生产计划达成率提升 23%，在制品库存下降 15%，质量异常响应时间缩短到 30 分钟以内。</p>'),(63,'<p>客户是工程机械制造企业，供应商数量多、采购品类复杂，订单确认和对账大量依赖邮件和电话。</p><p>项目组建设供应商协同平台，采购订单自动推送，供应商在线确认交期、上传发货信息和质量文件。到货后系统按规则自动对账，差异项进入异常处理流程。</p><p>平台上线后，采购订单确认周期由 3 天缩短到 1 天，对账工作量减少 70%，供应链透明度显著提升。</p>'),(64,'<p>公司 2026 年度生态伙伴大会日前举行，来自全国各地的渠道伙伴、实施伙伴和技术伙伴代表参加。</p><p>会上，公司发布了新一年的产品路线图和伙伴支持政策，包括联合方案打造、交付能力认证和区域市场联合推广等内容。多位伙伴分享了在制造、零售和医疗行业的项目经验。</p><p>公司表示，将持续完善伙伴体系，与生态伙伴形成能力互补，共同服务好客户的数字化需求。</p>'),(65,'<p>近日，公司向华南地区多家客户批量交付数据中台产品，覆盖制造和零售行业。</p><p>交付前，客户对数据接入、指标口径和权限体系进行了联合验收。实施团队协助客户完成十余个业务系统的数据接入，梳理了两百余项指标，并搭建了经营分析看板。</p><p>公司已安排客户成功团队持续跟进，提供数据质量监控和指标优化服务，帮助客户把数据真正用起来。</p>'),(66,'<p>低代码平台选型不能只看演示效果，建议从五个关键指标评估。</p><p>第一是建模能力：表单、流程、报表和数据模型是否完整，能否支撑复杂业务规则。第二是集成能力：是否提供开放 API、消息总线和常见系统连接器，能否与现有 ERP、OA 打通。第三是扩展性：是否支持自定义组件和代码扩展，避免平台能力成为天花板。第四是部署方式：是否支持私有化部署和信创环境。第五是生态：组件市场、行业模板和合作伙伴是否活跃。</p><p>建议选型时用真实业务场景做原型验证，并评估供应商的持续服务能力，而不是只比较价格。</p>'),(67,'<p>中小企业数字化面临预算有限、IT 人员不足的现实约束。低代码平台通过可视化建模和预置组件，让业务人员也能参与应用搭建，显著降低了开发门槛。</p><p>常见应用场景包括请假审批、费用报销、进销存、客户跟进和设备巡检等。企业可以先从一个高频痛点切入，两周内上线，验证效果后再逐步扩展。</p><p>需要提醒的是，低代码并不等于不需要治理。企业应同步建立数据标准、权限规范和发布流程，避免应用数量增长后出现新的信息孤岛。</p>'),(68,'<p>RustAPS 高级排产系统根据订单交期、设备产能、物料齐套和工艺约束，自动生成可执行的生产计划。</p><p>系统支持有限产能排产和模拟推演，计划员可以调整优先级、锁定关键订单，并实时查看调整后的交期影响。排产结果可下发到 MES 执行，执行偏差会反馈并触发重排。</p><p>通过提升计划准确性，企业可以缩短生产周期、降低在制品库存，并提高准时交付率。</p>'),(69,'<p>RustCRM 营销自动化模块帮助企业把分散的营销动作串成可衡量的旅程。</p><p>模块支持线索来源追踪、行为打分和自动分配，根据客户浏览、下载和活动参与情况判断意向。营销人员可以配置自动化旅程，通过邮件、短信和企微触达客户，并根据反馈调整后续动作。</p><p>系统提供渠道效果和转化分析，帮助市场团队优化投放策略和内容。</p>'),(70,'<p>RustHR 薪酬绩效模块支持多套薪酬方案、调薪流程和绩效核算，适用于不同岗位和用工形式。</p><p>绩效方面支持 KPI 和 OKR 两种模式，目标可逐级对齐，过程进展定期更新，考核结果与薪酬和晋升联动。薪酬核算自动读取考勤、绩效和社保数据，减少人工计算错误。</p><p>系统提供薪酬分析和人工成本看板，帮助管理者掌握人力投入产出情况。</p>'),(71,'<p>RustFin 费控报销模块覆盖事前申请、预算占用、移动报销、发票查验和财务审核全流程。</p><p>员工在移动端拍照上传发票，系统自动识别和查验，重复报销和超标准费用会被拦截。审批通过后生成凭证并推送付款，预算执行情况实时更新。</p><p>管理者可以按部门、项目和时间维度查看费用分析，及时发现异常支出。</p>'),(72,'<p>RustBI 大屏可视化套件面向指挥中心、展厅和经营监控场景，提供地图、指标卡、趋势图、关系图等丰富的可视化组件。</p><p>套件内置多套行业大屏模板，支持拖拽布局和主题定制，数据可实时接入业务系统和数据中台。大屏适配不同分辨率，并支持定时轮播和多屏联动。</p><p>配套的发布和权限管理，让不同部门可以维护各自的大屏内容。</p>'),(73,'<p>我们提供独立的软件测试与质量保障服务，覆盖功能测试、接口测试、自动化测试、性能测试和安全测试。</p><p>项目启动时先梳理测试范围和风险点，制定测试计划与用例；执行阶段通过缺陷管理工具跟踪问题闭环；上线前完成回归测试和性能压测，输出质量报告。</p><p>对于长期项目，我们会帮助客户搭建自动化测试框架和持续集成流水线，把质量保障前置到研发过程中。</p>'),(74,'<p>客户是新能源电池制造企业，产线自动化程度高但数据分散在 MES、设备和质量系统中，缺乏统一分析。</p><p>项目组建设数据中台，接入十余个系统的数据，统一物料、产线和工序编码，梳理关键指标并建立指标字典。基于中台搭建良率分析、设备效率和能耗看板。</p><p>项目上线后，良率异常定位时间由数小时缩短到十分钟，管理层可以按产线、班次和批次查看经营数据，为持续改进提供支撑。</p>'),(75,'<p>客户是精密模具企业，订单多、项目周期长，报价和试模记录分散在 Excel 中，进度跟踪困难。</p><p>项目组基于低代码平台搭建了报价管理、项目进度和试模记录三个应用，业务人员经过培训后可以自行调整表单和流程。应用与 ERP 的订单和成本数据对接。</p><p>上线两个月内完成全部应用搭建，模具项目进度透明化，报价周期缩短 40%，客户对交付节点的满意度明显提升。</p>');
/*!40000 ALTER TABLE `archive_data` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_drafts`
--

DROP TABLE IF EXISTS `archive_drafts`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_drafts` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `parent_id` bigint(20) NOT NULL DEFAULT '0',
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(190) NOT NULL DEFAULT '',
  `seo_title` varchar(250) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `module_id` int(10) unsigned NOT NULL DEFAULT '1',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `place_id` int(10) NOT NULL DEFAULT '0',
  `views` int(10) unsigned NOT NULL DEFAULT '0',
  `comment_count` int(10) unsigned NOT NULL DEFAULT '0',
  `favorite_count` int(10) unsigned NOT NULL DEFAULT '0',
  `images` text,
  `template` varchar(250) NOT NULL DEFAULT '',
  `canonical_url` varchar(250) NOT NULL DEFAULT '',
  `fixed_link` varchar(190) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `price` bigint(20) NOT NULL DEFAULT '0',
  `stock` bigint(20) NOT NULL DEFAULT '9999999',
  `read_level` int(10) NOT NULL DEFAULT '0',
  `password` varchar(32) NOT NULL DEFAULT '',
  `sort` int(10) unsigned NOT NULL DEFAULT '0',
  `has_pseudo` tinyint(1) NOT NULL DEFAULT '0',
  `keyword_id` bigint(20) NOT NULL DEFAULT '0',
  `origin_url` varchar(190) NOT NULL DEFAULT '',
  `origin_title` varchar(190) NOT NULL DEFAULT '',
  `origin_id` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_archive_drafts_parent_id` (`parent_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_category_created_time` (`category_id`,`created_time`),
  KEY `idx_module_created_time` (`module_id`,`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_archive_drafts_title` (`title`),
  KEY `idx_archive_drafts_url_token` (`url_token`),
  KEY `idx_archive_drafts_place_id` (`place_id`),
  KEY `idx_views` (`views`),
  KEY `idx_archive_drafts_fixed_link` (`fixed_link`),
  KEY `idx_archive_drafts_user_id` (`user_id`),
  KEY `idx_sort` (`sort`),
  KEY `idx_origin_url` (`origin_url`),
  KEY `idx_origin_title` (`origin_title`)
) ENGINE=InnoDB AUTO_INCREMENT=77 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_drafts`
--

LOCK TABLES `archive_drafts` WRITE;
/*!40000 ALTER TABLE `archive_drafts` DISABLE KEYS */;
INSERT INTO `archive_drafts` (`id`, `parent_id`, `created_time`, `updated_time`, `title`, `seo_title`, `url_token`, `keywords`, `description`, `module_id`, `category_id`, `place_id`, `views`, `comment_count`, `favorite_count`, `images`, `template`, `canonical_url`, `fixed_link`, `user_id`, `price`, `stock`, `read_level`, `password`, `sort`, `has_pseudo`, `keyword_id`, `origin_url`, `origin_title`, `origin_id`, `status`) VALUES (49,0,1792642150,1792642150,'会员发布测试文章','','hui-yuan-fa-bu-ce-shi-wen-zhang','','来自会员发布页的测试内容',1,13,0,0,0,0,NULL,'','','',1,0,9999999,0,'',0,0,0,'','',0,0),(50,0,1792643416,1792643416,'会员发布页功能对齐测试','','member-publish-align-test','会员发布,富文本,对齐后台','这是通过会员中心发布页提交的测试内容，字段与后台添加文档一致。',1,13,0,0,0,0,'{\"uploads/202610/22/63f5d82aae0f0cec.webp\"}','','','',1,0,9999999,0,'',0,0,0,'','',0,0),(51,0,1792643638,1792643638,'浏览器提交：产品发布测试 1792643638048','','liu-lan-qi-ti-jiao-chan-pin-fa-bu-ce-shi-1792643638048','产品,发布,测试','这是产品发布的正文内容。',2,3,0,0,0,0,'{}','','','',1,19900,50,0,'',0,0,0,'','',0,0),(52,0,1792648512,1792648512,'10000','','10000','41111111111','4111111111',2,3,0,0,0,0,'{}','','','',2,2555500,100,0,'',0,0,0,'','',0,0);
/*!40000 ALTER TABLE `archive_drafts` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_favorites`
--

DROP TABLE IF EXISTS `archive_favorites`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_favorites` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `sku_id` bigint(20) unsigned NOT NULL DEFAULT '0',
  `user_id` bigint(20) NOT NULL DEFAULT '0',
  `created_time` bigint(20) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_archive_collection_id` (`archive_id`,`user_id`),
  KEY `idx_archive_favorites_sku_id` (`sku_id`),
  KEY `idx_archive_favorites_user_id` (`user_id`),
  KEY `idx_created_time` (`created_time`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_favorites`
--

LOCK TABLES `archive_favorites` WRITE;
/*!40000 ALTER TABLE `archive_favorites` DISABLE KEYS */;
INSERT INTO `archive_favorites` (`id`, `archive_id`, `sku_id`, `user_id`, `created_time`) VALUES (1,38,0,1,1792640685),(2,44,0,2,1792647417);
/*!40000 ALTER TABLE `archive_favorites` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_flags`
--

DROP TABLE IF EXISTS `archive_flags`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_flags` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `flag` char(1) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_archive_id_flag` (`archive_id`,`flag`),
  KEY `idx_archive_flags_flag` (`flag`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_flags`
--

LOCK TABLES `archive_flags` WRITE;
/*!40000 ALTER TABLE `archive_flags` DISABLE KEYS */;
INSERT INTO `archive_flags` (`id`, `archive_id`, `flag`) VALUES (2,50,'c'),(1,50,'h');
/*!40000 ALTER TABLE `archive_flags` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archive_relations`
--

DROP TABLE IF EXISTS `archive_relations`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archive_relations` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `relation_id` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_archive_relation_id` (`archive_id`,`relation_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archive_relations`
--

LOCK TABLES `archive_relations` WRITE;
/*!40000 ALTER TABLE `archive_relations` DISABLE KEYS */;
/*!40000 ALTER TABLE `archive_relations` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `archives`
--

DROP TABLE IF EXISTS `archives`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `archives` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `parent_id` bigint(20) NOT NULL DEFAULT '0',
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(190) NOT NULL DEFAULT '',
  `seo_title` varchar(250) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `module_id` int(10) unsigned NOT NULL DEFAULT '1',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `place_id` int(10) NOT NULL DEFAULT '0',
  `views` int(10) unsigned NOT NULL DEFAULT '0',
  `comment_count` int(10) unsigned NOT NULL DEFAULT '0',
  `favorite_count` int(10) unsigned NOT NULL DEFAULT '0',
  `images` text,
  `template` varchar(250) NOT NULL DEFAULT '',
  `canonical_url` varchar(250) NOT NULL DEFAULT '',
  `fixed_link` varchar(190) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `price` bigint(20) NOT NULL DEFAULT '0',
  `stock` bigint(20) NOT NULL DEFAULT '9999999',
  `read_level` int(10) NOT NULL DEFAULT '0',
  `password` varchar(32) NOT NULL DEFAULT '',
  `sort` int(10) unsigned NOT NULL DEFAULT '0',
  `has_pseudo` tinyint(1) NOT NULL DEFAULT '0',
  `keyword_id` bigint(20) NOT NULL DEFAULT '0',
  `origin_url` varchar(190) NOT NULL DEFAULT '',
  `origin_title` varchar(190) NOT NULL DEFAULT '',
  `origin_id` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_archives_parent_id` (`parent_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_category_created_time` (`category_id`,`created_time`),
  KEY `idx_module_created_time` (`module_id`,`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_archives_title` (`title`),
  KEY `idx_archives_url_token` (`url_token`),
  KEY `idx_archives_place_id` (`place_id`),
  KEY `idx_views` (`views`),
  KEY `idx_archives_fixed_link` (`fixed_link`),
  KEY `idx_archives_user_id` (`user_id`),
  KEY `idx_sort` (`sort`),
  KEY `idx_origin_url` (`origin_url`),
  KEY `idx_origin_title` (`origin_title`)
) ENGINE=InnoDB AUTO_INCREMENT=77 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `archives`
--

LOCK TABLES `archives` WRITE;
/*!40000 ALTER TABLE `archives` DISABLE KEYS */;
INSERT INTO `archives` (`id`, `parent_id`, `created_time`, `updated_time`, `title`, `seo_title`, `url_token`, `keywords`, `description`, `module_id`, `category_id`, `place_id`, `views`, `comment_count`, `favorite_count`, `images`, `template`, `canonical_url`, `fixed_link`, `user_id`, `price`, `stock`, `read_level`, `password`, `sort`, `has_pseudo`, `keyword_id`, `origin_url`, `origin_title`, `origin_id`) VALUES (1,0,1694739182,1694739182,'RustCMS 发布 2026 春季版本，工作流引擎全面升级','RustCMS 发布 2026 春季版本，工作流引擎全面升级','mou-mou-ji-xie-zhi-zao-gong-si-yu-guo-nei-zhi-ming-qi-ye-he-zuo-kai-fa-xin-yi-dai-ji-qi-ren','RustCMS,版本发布,工作流引擎,低代码','新版本重构了工作流引擎，支持可视化编排、条件分支和跨系统回调，审批效率提升约 40%。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(2,0,1694739313,1694739313,'公司获评省级“专精特新”软件企业','公司获评省级“专精特新”软件企业','mou-mou-ji-xie-zhi-zao-gong-si-rong-huo-guo-jia-gao-xin-ji-shu-qi-ye-cheng-hao','专精特新,软件企业,资质认定','凭借在企业级软件领域的持续投入和产品创新，公司入选省级“专精特新”中小企业名单。',1,13,0,2,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(3,0,1694739434,1694739434,'RustCMS 低代码平台 3.0 发布，交付效率提升 50%','RustCMS 低代码平台 3.0 发布，交付效率提升 50%','mou-mou-ji-xie-zhi-zao-gong-si-tui-chu-quan-xin-zi-dong-hua-sheng-chan-xian-ti-gao-sheng-chan-xiao-l','低代码,平台发布,表单引擎,交付效率','3.0 版本重构表单引擎和数据模型设计器，支持复杂业务规则与多端发布，项目交付周期显著缩短。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(4,0,1694739614,1694739614,'公司与云厂商达成生态合作，联合推出行业云方案','公司与云厂商达成生态合作，联合推出行业云方案','gong-si-cheng-gong-yan-fa-chu-gao-xiao-jie-neng-de-feng-li-fa-dian-she-bei','生态合作,行业云,云计算,联合方案','双方将在产品适配、联合方案和区域市场拓展方面展开合作，推出面向制造行业的云上数字化方案。',1,13,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(5,0,1694739717,1694739717,'公司与高校共建软件工程联合实验室','公司与高校共建软件工程联合实验室','gong-si-yu-guo-ji-zhi-ming-qi-che-zhi-zao-shang-qian-shu-he-zuo-xie-yi-gong-tong-kai-fa-qi-che-ling','产学研,联合实验室,软件工程,人才培养','联合实验室将围绕低代码引擎、软件质量与智能编程方向开展研究和人才联合培养。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(6,0,1694739834,1694739834,'公司参加 2026 中国国际软件博览会','公司参加 2026 中国国际软件博览会','gong-si-can-jia-guo-ji-ji-xie-zhi-zao-zhan-lan-hui-zhan-shi-zui-xin-chan-pin-he-ji-shu','软件博览会,展会,数字化成果','公司携 ERP、低代码平台和数据中台三条产品线参展，现场演示企业数字化全流程方案。',1,13,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(7,0,1694739955,1694739955,'公司通过 CMMI 3 级评估，研发管理再上台阶','公司通过 CMMI 3 级评估，研发管理再上台阶','gong-si-huo-de-duo-xiang-zhuan-li-cheng-wei-xing-ye-nei-ji-shu-chuang-xin-ling-jun-qi-ye','CMMI,研发管理,过程改进,软件成熟度','CMMI 3 级评估的通过，标志着公司在需求、开发、测试和项目管理方面形成了标准化体系。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(8,0,1694740035,1694740035,'公司通过 ISO 27001 信息安全管理体系认证','公司通过 ISO 27001 信息安全管理体系认证','gong-si-tong-guo-iso9001-zhi-liang-guan-li-ti-xi-ren-zheng-ti-sheng-chan-pin-zhi-liang-he-fu-wu-shui','ISO27001,信息安全,数据保护,合规','认证覆盖软件研发、交付与运维服务全过程，为客户数据安全提供体系化保障。',1,13,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(9,0,1694740129,1694740129,'公司荣获省科技进步奖，低代码引擎获认可','公司荣获省科技进步奖，低代码引擎获认可','gong-si-rong-huo-sheng-ji-ke-ji-jin-bu-jiang-biao-zhang-qi-zai-ke-ji-chuang-xin-fang-mian-de-tu-chu','科技进步奖,低代码引擎,技术创新','公司参与完成的“面向企业应用的低代码开发关键技术”项目获得省科技进步三等奖。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(10,0,1694740228,1694740228,'公司启动“工程师成长计划”，完善技术人才培养','公司启动“工程师成长计划”，完善技术人才培养','gong-si-kai-zhan-yuan-gong-pei-xun-ji-hua-ti-sheng-yuan-gong-ji-neng-shui-ping-he-zhi-ye-su-yang','人才培养,工程师,技术培训,职业发展','计划通过导师制、技术分享和认证激励，帮助工程师在架构、数据和业务领域持续成长。',1,13,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(11,0,1694740365,1694740365,'公司举办 2026 程序员节技术嘉年华','公司举办 2026 程序员节技术嘉年华','gong-si-zu-zhi-da-xing-tuan-jian-huo-dong-qiu-you-zhang-jiang-bian-yuan-gong-tuan-jian-qi-le-rong-ro','程序员节,企业文化,技术活动,团建','活动包括技术擂台、开源项目分享和团队拓展，近两百名员工参与。',1,13,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(12,0,1694740580,1694740580,'某汽车零部件集团 ERP 升级项目','某汽车零部件集团 ERP 升级项目','mou-mou-qi-che-gong-si','汽车零部件,ERP升级,集团管控,案例','为多基地汽车零部件集团完成 ERP 升级，实现财务业务一体化和集团统一管控。',3,15,0,2,0,0,'{\"/uploads/software/theme-10.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(13,0,1694740724,1694740724,'某航空制造企业 MES 实施项目','某航空制造企业 MES 实施项目','mou-mou-hang-kong-gong-ye-ji-tuan','航空制造,MES,质量追溯,生产执行,案例','为航空零部件企业实施 MES，实现工序级追溯和生产过程透明化管理。',3,15,0,1,0,0,'{\"/uploads/software/theme-06.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(14,0,1694740844,1694740844,'某医疗器械公司质量追溯系统','某医疗器械公司质量追溯系统','mou-mou-yi-liao-qi-xie-gong-si','医疗器械,质量追溯,UDI,合规,案例','建设符合监管要求的质量追溯系统，实现从原料到患者使用的全链路追溯。',3,15,0,1,0,0,'{\"/uploads/software/theme-10.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(15,0,1694741147,1694741147,'某家电集团全渠道中台项目','某家电集团全渠道中台项目','mou-mou-jia-dian-zhi-zao-gong-si','家电,全渠道,中台,会员,订单,案例','整合线上商城、门店和电商平台，统一商品、会员、订单和库存。',3,15,0,1,0,0,'{\"/uploads/software/theme-06.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(16,0,1694741248,1694741248,'某建筑集团协同办公平台','某建筑集团协同办公平台','mou-mou-jian-zhu-gong-si','建筑集团,协同办公,OA,项目管控,案例','为建筑集团搭建协同办公平台，覆盖集团、分子公司和项目部三级组织。',3,15,0,5,0,0,'{\"/uploads/software/theme-10.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(17,0,1694741433,1694741433,'制造企业数字化转型解决方案','制造企业数字化转型解决方案','zi-dong-hua-sheng-chan-xian-jie-jue-fang-an','制造业,数字化转型,ERP,MES,数据中台','以 ERP、MES 和数据中台为核心，帮助制造企业打通业务与数据，提升交付与决策效率。',1,12,0,2,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(18,0,1694741555,1694741555,'智能客服与知识库解决方案','智能客服与知识库解决方案','ji-qi-ren-ying-yong-jie-jue-fang-an','智能客服,知识库,大模型,工单','结合知识库、工单和大模型能力，构建可自助、可转人工、可沉淀知识的客户服务体系。',1,12,0,0,0,0,'{\"/uploads/software/theme-03.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(19,0,1694741696,1694741696,'能源行业数据采集与分析解决方案','能源行业数据采集与分析解决方案','neng-yuan-she-bei-kai-fa-jie-jue-fang-an','能源行业,数据采集,时序数据,分析','面向能源企业提供设备数据采集、时序存储、实时监控和能效分析能力。',1,12,0,0,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(20,0,1694741781,1694741781,'供应链协同解决方案','供应链协同解决方案','ling-bu-jian-zhi-zao-jie-jue-fang-an','供应链协同,采购,供应商,对账','打通采购、库存、供应商与财务，实现订单协同、库存可视和对账自动化。',1,12,0,0,0,0,'{\"/uploads/software/theme-03.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(21,0,1694741893,1694741893,'企业数字化人才培养解决方案','企业数字化人才培养解决方案','ji-shu-pei-xun-jie-jue-fang-an','数字化人才,培训,认证,课程','通过课程、实训和认证相结合，帮助企业建立懂业务、懂系统的数字化人才梯队。',1,12,0,0,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(22,0,1694742140,1694745939,'云原生架构下的高可用实践','云原生架构下的高可用实践','mou-mou-shu-kong-ji-chuang','云原生,高可用,容器,容灾','从多副本、健康检查、熔断限流到多可用区部署，介绍云原生高可用架构的落地要点。',1,14,0,5,0,0,'{\"/uploads/software/theme-05.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(23,0,1694742244,1694745971,'微服务拆分的五个关键决策','微服务拆分的五个关键决策','mou-mou-zuan-chuang','微服务,架构拆分,领域驱动,治理','拆分前需要回答边界、数据、通信、部署和治理五个问题，避免为了拆而拆。',1,14,0,0,0,0,'{\"/uploads/software/theme-06.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(24,0,1694742357,1694745992,'前端性能优化的十个技巧','前端性能优化的十个技巧','mou-mou-xi-chuang','前端性能,加载优化,缓存,渲染','从资源体积、加载策略、缓存和渲染四个方面，整理十项可落地的前端性能优化技巧。',1,14,0,6,0,0,'{\"/uploads/software/theme-05.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(25,0,1694742428,1694742428,'数据中台建设中的数据治理','数据中台建设中的数据治理','mou-mou-zi-dong-zhuang-pei-xian','数据中台,数据治理,指标,主数据','数据治理决定数据中台能否持续产生价值，重点包括标准、质量、安全和运营四方面。',1,14,0,0,0,0,'{\"/uploads/software/theme-06.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(26,0,1694742544,1694742544,'AI 辅助编程在研发团队中的落地','AI 辅助编程在研发团队中的落地','mou-mou-ji-qi-ren','AI编程,研发效率,代码审查,规范','从场景选择、规范约束到质量把关，介绍研发团队引入 AI 辅助编程的实践经验。',1,14,0,2,0,0,'{\"/uploads/software/theme-05.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(27,0,1694743004,1694743004,'企业门户定制开发服务','企业门户定制开发服务','precision-g-100-chi-lun','企业门户,定制开发,官网,门户集成','提供企业官网、内部门户和供应商门户的定制设计与开发服务。',2,6,0,0,0,0,'{\"/uploads/software/theme-10.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(28,0,1694743082,1694743082,'小程序与 H5 定制开发','小程序与 H5 定制开发','dependable-plus-luo-shuan','小程序,H5,移动应用,微信','提供微信小程序、企业微信应用和 H5 活动的设计与开发服务。',2,6,0,0,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(29,0,1694743162,1694743162,'系统集成与接口开发','系统集成与接口开发','optima-bearing-series-zhou-cheng','系统集成,接口开发,API,中间件','打通 ERP、OA、CRM、MES 等系统，提供接口开发与集成治理服务。',2,6,0,0,0,0,'{\"/uploads/software/theme-10.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(30,0,1694743265,1694743265,'遗留系统迁移与重构','遗留系统迁移与重构','efficient-drive-coupling-chuan-dong-jian','系统迁移,重构,数据库迁移,技术升级','为老旧系统提供评估、迁移和重构服务，降低技术债务与运行风险。',2,6,0,1,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(31,0,1694744437,1694744437,'RustBI 商业智能平台','RustBI 商业智能平台','pro-series-screwdriver-luo-si-dao','BI,数据分析,可视化,报表','RustBI 提供数据建模、可视化分析和自助报表，帮助业务人员自主探索数据。',2,5,0,0,0,0,'{\"/uploads/software/theme-09.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(32,0,1694744503,1694744503,'RustReport 报表设计器','RustReport 报表设计器','masterart-scriber-diao-ke-dao','报表,报表设计,套打,导出','支持复杂报表设计、套打、分组汇总和多格式导出，满足企业报表需求。',2,5,0,0,0,0,'{\"/uploads/software/theme-03.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(33,0,1694744585,1694744585,'RustData 数据集成平台','RustData 数据集成平台','airdrill-3000-feng-dong-zuan','数据集成,ETL,数据同步,接口','支持多源数据接入、实时同步、任务调度和数据质量管理。',2,5,0,0,0,0,'{\"/uploads/software/theme-09.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(34,0,1694744656,1694744656,'RustMetric 指标管理平台','RustMetric 指标管理平台','millimeter-6000-qian-fen-chi','指标管理,指标字典,口径,数据治理','统一管理指标定义、计算口径和责任人，避免同名不同义的数据问题。',2,5,0,0,0,0,'{\"/uploads/software/theme-03.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(35,0,1694744768,1694744768,'RustOA 协同办公平台','RustOA 协同办公平台','eclipse-automator','OA,协同办公,审批,公文,门户','RustOA 提供门户、审批、公文、会议和知识管理，打造统一的协同办公入口。',2,4,0,0,0,0,'{\"/uploads/software/theme-08.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(36,0,1694744833,1694744833,'RustFlow 工作流引擎','RustFlow 工作流引擎','medtech-automator','工作流引擎,流程编排,审批,低代码','可视化流程编排，支持条件分支、并行网关、会签和跨系统回调，可独立部署或嵌入业务系统。',2,4,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(37,0,1694744894,1694744894,'RustDocs 企业文档中心','RustDocs 企业文档中心','orbital-welding-robot','文档管理,知识库,版本控制,权限','集中管理企业文档与知识，支持在线预览、版本控制和细粒度权限。',2,4,0,1,0,0,'{\"/uploads/software/theme-08.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(38,0,1694744956,1694744956,'RustMeet 视频会议系统','RustMeet 视频会议系统','fieldbotics-5000','视频会议,在线协作,屏幕共享,会议纪要','支持高清音视频、屏幕共享、会议纪要和日程联动，适合企业内外沟通。',2,4,0,1,1,1,'{\"/uploads/software/theme-02.png\"}','','','',1,19900,9999999,0,'',0,0,0,'','',0),(39,0,1694745051,1694746110,'制造业 MES 解决方案','制造业 MES 解决方案','masterfab-6000-ji-chuang','制造业,MES,生产管理,数字化车间','面向制造企业的生产执行方案，覆盖计划排产、工序报工、质量追溯和设备管理。',2,7,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(40,0,1694745119,1694746102,'零售业全渠道中台方案','零售业全渠道中台方案','centurymill-7000-ji-chuang','零售,全渠道,中台,会员,订单','整合线上商城、门店和渠道订单，统一会员、商品与库存，实现全渠道运营。',2,7,0,0,0,0,'{\"/uploads/software/theme-05.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(41,0,1694745177,1694746124,'医疗行业信息化方案','医疗行业信息化方案','glidestream-8000-ji-chuang','医疗信息化,医院管理,患者服务,数据安全','面向医疗机构提供患者服务、运营管理和数据安全方案，兼顾合规与体验。',2,7,0,0,0,0,'{\"/uploads/software/theme-01.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(42,0,1694745271,1694746087,'教育行业智慧校园平台','教育行业智慧校园平台','vortex-5000','智慧校园,教育信息化,教务,家校','覆盖教务、学生、教师和家校协同，打造一体化智慧校园平台。',2,7,0,6,0,0,'{\"/uploads/software/theme-05.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(43,0,1694745382,1694745382,'RustMES 生产执行系统','RustMES 生产执行系统','drillmaster-3000-zuan-chuang','MES,生产执行,工单,报工,追溯','以工单为核心的车间执行系统，支持报工、质检、追溯和看板管理。',2,8,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(44,0,1694745470,1694746077,'RustWMS 仓储管理系统','RustWMS 仓储管理系统','drillspark-4000-zuan-chuang','WMS,仓储管理,库存,条码,库位','支持多仓库、多货主和条码管理，实现收货、上架、拣货和盘点全流程数字化。',2,8,0,2,0,1,'{\"/uploads/software/theme-06.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(45,0,1694745534,1694746061,'RustQMS 质量管理系统','RustQMS 质量管理系统','drillmaster-pro-5000-zuan-chuang','QMS,质量管理,检验,不合格品,追溯','覆盖来料、过程、成品检验和不合格品处理，建立完整的质量追溯体系。',2,8,0,0,0,0,'{\"/uploads/software/theme-02.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(46,0,1694745714,1694745714,'RustCRM 客户关系管理平台','RustCRM 客户关系管理平台','mastermill-7000-xi-chuang','CRM,客户管理,商机,销售,服务','管理客户、联系人、商机和售后服务，帮助销售团队提升跟进效率。',2,9,0,0,0,0,'{\"/uploads/software/theme-07.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(47,0,1694745777,1694746029,'RustHR 人力资源管理系统','RustHR 人力资源管理系统','turntech-6000-che-chuang','HR,人事管理,组织,考勤,招聘','覆盖组织、人事、考勤、招聘和员工自助，提升人力资源管理效率。',2,10,0,1,0,0,'{\"/uploads/software/theme-04.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(48,0,1694745839,1694746048,'RustFin 财务共享平台','RustFin 财务共享平台','grindmaster-8000','财务共享,核算,对账,报表,集团','面向集团企业提供财务共享服务，统一核算、对账和报表流程。',2,11,0,1,0,0,'{\"/uploads/software/theme-09.png\"}','','','',0,0,9999999,0,'',0,0,0,'','',0),(56,0,1794034488,1794034488,'公司通过 ISO 9001:2015 质量管理体系年度审核','公司通过 ISO 9001:2015 质量管理体系年度审核','iso-9001-audit-2026','ISO9001,质量管理,年度审核,软件交付','审核覆盖需求、研发、实施与运维全过程，公司质量管理体系的持续有效性获得肯定。',1,13,0,12,0,0,'{\"/uploads/software/theme-01.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(57,0,1794034488,1794034488,'RustData 数据中台完成压测并交付客户','RustData 数据中台完成压测并交付客户','new-cnc-center-delivered','数据中台,压力测试,交付,数据集成','平台在亿级数据量下完成稳定性压测，首批客户已完成上线运行。',1,13,0,8,0,0,'{\"/uploads/software/theme-02.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(58,0,1794034488,1794034488,'公司亮相 2026 数字中国建设峰会','公司亮相 2026 数字中国建设峰会','ciie-2026-expo','数字中国,峰会,数字化转型,成果展示','公司展示数据中台和低代码平台在政务与企业数字化中的应用成果。',1,13,0,15,0,0,'{\"/uploads/software/theme-01.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(59,0,1794034521,1794034521,'RustERP 企业管理平台','RustERP 企业管理平台','vmc-850','ERP,企业管理,财务,供应链,生产','RustERP 覆盖财务、供应链、生产与销售，帮助企业建立统一的经营管理平台。',2,3,0,23,0,0,'{\"/uploads/software/theme-07.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(60,0,1794034521,1794034521,'RustERP 财务核算模块','RustERP 财务核算模块','ck-6150','财务核算,总账,应收应付,报表','支持总账、应收应付、固定资产和财务报表，与业务单据实时联动。',2,3,0,17,0,0,'{\"/uploads/software/theme-01.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(61,0,1794034521,1794034521,'RustOA 移动办公套件','RustOA 移动办公套件','aps-200','移动办公,APP,小程序,待办','提供 APP 和小程序入口，支持待办审批、考勤打卡、通讯录和消息推送。',2,4,0,31,0,0,'{\"/uploads/software/theme-08.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(62,0,1794034521,1794034521,'汽车零部件企业智能工厂数字化项目','汽车零部件企业智能工厂数字化项目','auto-parts-line-project','智能工厂,数字化,ERP,MES,数据采集,案例','以 ERP 与 MES 为核心，打通计划、生产、质量和设备数据，建设数字化车间。',3,15,0,44,0,0,'{\"/uploads/software/theme-10.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(63,0,1794034521,1794034521,'工程机械企业供应链协同平台','工程机械企业供应链协同平台','flexible-manufacturing-cell','工程机械,供应链协同,供应商,采购,案例','连接数百家供应商，实现订单协同、交期确认和自动对账。',3,15,0,38,0,0,'{\"/uploads/software/theme-06.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(64,0,1794034701,1794034701,'公司举办 2026 年度生态伙伴大会','公司举办 2026 年度生态伙伴大会','supplier-quality-meeting-2026','生态伙伴,合作伙伴大会,渠道,生态','来自全国各地的渠道与实施伙伴齐聚一堂，共同探讨行业数字化机会与合作模式。',1,13,0,9,0,0,'{\"/uploads/software/theme-01.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(65,0,1794034701,1794034701,'数据中台产品批量交付华南客户','数据中台产品批量交付华南客户','cnc-batch-delivery-south','数据中台,批量交付,华南客户,数据治理','多套数据中台完成验收并交付华南地区客户，覆盖制造与零售行业。',1,13,0,11,0,0,'{\"/uploads/software/theme-02.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(66,0,1794034701,1794034701,'如何选择适合企业的低代码平台：五个关键指标','如何选择适合企业的低代码平台：五个关键指标','how-to-choose-vmc','低代码,选型,平台评估,扩展能力','从建模能力、集成能力、扩展性、私有化部署和生态五个维度评估低代码平台。',1,14,0,19,0,0,'{\"/uploads/software/theme-05.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(67,0,1794034701,1794034701,'低代码平台在中小企业数字化中的应用','低代码平台在中小企业数字化中的应用','flexible-cell-application','低代码,中小企业,数字化,应用搭建','中小企业可以借助低代码平台，以较低成本快速搭建审批、进销存和客户管理等应用。',1,12,0,26,0,0,'{\"/uploads/software/theme-04.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(68,0,1794034701,1794034701,'RustAPS 高级排产系统','RustAPS 高级排产系统','z-5140','APS,高级排产,产能,交期,优化','基于产能、物料和交期约束进行自动排产，提升设备利用率和准时交付率。',2,8,0,14,0,0,'{\"/uploads/software/theme-06.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(69,0,1794034701,1794034701,'RustCRM 营销自动化模块','RustCRM 营销自动化模块','gx-3018','营销自动化,线索,培育,触达,转化','通过线索打分、自动化旅程和多渠道触达，提升营销转化效率。',2,9,0,21,0,0,'{\"/uploads/software/theme-03.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(70,0,1794034701,1794034701,'RustHR 薪酬绩效模块','RustHR 薪酬绩效模块','ck-6180','薪酬,绩效,KPI,OKR,核算','支持多套薪酬方案和 KPI/OKR 绩效管理，实现核算与考核一体化。',2,10,0,13,0,0,'{\"/uploads/software/theme-08.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(71,0,1794034701,1794034701,'RustFin 费控报销模块','RustFin 费控报销模块','mg-7120','费控,报销,预算,发票,审批','支持预算控制、发票查验和移动报销，实现费用全流程管控。',2,11,0,16,0,0,'{\"/uploads/software/theme-05.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(72,0,1794034701,1794034701,'RustBI 大屏可视化套件','RustBI 大屏可视化套件','tm-12','数据大屏,可视化,指挥中心,实时','提供丰富的可视化组件和大屏模板，支持实时数据接入和多屏适配。',2,5,0,27,0,0,'{\"/uploads/software/theme-03.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(73,0,1794034701,1794034701,'软件测试与质量保障服务','软件测试与质量保障服务','bs-4010','软件测试,质量保障,自动化测试,性能测试','提供功能测试、接口测试、自动化测试和性能测试服务，保障系统交付质量。',2,6,0,18,0,0,'{\"/uploads/software/theme-10.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(74,0,1794034701,1794034701,'新能源电池企业数据中台项目','新能源电池企业数据中台项目','battery-tray-line-project','新能源,电池,数据中台,数据治理,案例','整合生产、质量和设备数据，建设指标体系与经营分析看板。',3,15,0,35,0,0,'{\"/uploads/software/theme-10.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0),(75,0,1794034701,1794034701,'精密模具企业低代码平台应用','精密模具企业低代码平台应用','five-axis-mold-case','精密模具,低代码,应用搭建,数字化,案例','用低代码平台快速搭建报价、项目进度和试模管理应用，缩短交付周期。',3,15,0,29,0,0,'{\"/uploads/software/theme-06.png\"}','','',NULL,0,0,9999999,0,'',0,0,0,'','',0);
/*!40000 ALTER TABLE `archives` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `article`
--

DROP TABLE IF EXISTS `article`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `article` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=68 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `article`
--

LOCK TABLES `article` WRITE;
/*!40000 ALTER TABLE `article` DISABLE KEYS */;
INSERT INTO `article` (`id`) VALUES (1),(2),(3),(4),(5),(6),(7),(8),(9),(10),(11),(17),(18),(19),(20),(21),(22),(23),(24),(25),(26),(56),(57),(58),(64),(65),(66),(67);
/*!40000 ALTER TABLE `article` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `attachments`
--

DROP TABLE IF EXISTS `attachments`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `attachments` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `file_name` varchar(250) NOT NULL DEFAULT '',
  `file_location` varchar(250) NOT NULL DEFAULT '',
  `file_size` bigint(20) unsigned NOT NULL DEFAULT '0',
  `file_md5` varchar(32) NOT NULL DEFAULT '',
  `width` int(10) unsigned NOT NULL DEFAULT '0',
  `height` int(10) unsigned NOT NULL DEFAULT '0',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `is_image` tinyint(1) NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `watermark` tinyint(1) NOT NULL DEFAULT '0',
  `is_remote` tinyint(1) NOT NULL DEFAULT '0',
  `logo` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_attachments_file_md5` (`file_md5`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_attachments_user_id` (`user_id`),
  KEY `idx_category_id` (`category_id`),
  KEY `idx_status` (`status`)
) ENGINE=InnoDB AUTO_INCREMENT=8 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `attachments`
--

LOCK TABLES `attachments` WRITE;
/*!40000 ALTER TABLE `attachments` DISABLE KEYS */;
/*!40000 ALTER TABLE `attachments` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `carts`
--

DROP TABLE IF EXISTS `carts`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `carts` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `quantity` int(10) NOT NULL DEFAULT '1',
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_user_archive` (`user_id`,`archive_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `carts`
--

LOCK TABLES `carts` WRITE;
/*!40000 ALTER TABLE `carts` DISABLE KEYS */;
INSERT INTO `carts` (`id`, `created_time`, `updated_time`, `user_id`, `archive_id`, `quantity`) VALUES (2,1792642354,1792642354,1,37,2),(3,1792647415,1792647415,2,44,3);
/*!40000 ALTER TABLE `carts` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `case`
--

DROP TABLE IF EXISTS `case`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `case` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=76 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `case`
--

LOCK TABLES `case` WRITE;
/*!40000 ALTER TABLE `case` DISABLE KEYS */;
INSERT INTO `case` (`id`) VALUES (12),(13),(14),(15),(16),(62),(63),(74),(75);
/*!40000 ALTER TABLE `case` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `categories`
--

DROP TABLE IF EXISTS `categories`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `categories` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `seo_title` varchar(250) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `content` longtext,
  `module_id` int(10) unsigned NOT NULL DEFAULT '0',
  `parent_id` int(10) unsigned NOT NULL DEFAULT '0',
  `type` int(10) unsigned NOT NULL DEFAULT '0',
  `sort` int(10) unsigned NOT NULL DEFAULT '99',
  `template` varchar(250) NOT NULL DEFAULT '',
  `detail_template` varchar(250) NOT NULL DEFAULT '',
  `is_inherit` int(1) unsigned NOT NULL DEFAULT '0',
  `images` text,
  `logo` varchar(250) NOT NULL DEFAULT '',
  `extra` longtext,
  `archive_count` int(10) unsigned NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_categories_url_token` (`url_token`),
  KEY `idx_module_id` (`module_id`),
  KEY `idx_parent_id` (`parent_id`),
  KEY `idx_type` (`type`),
  KEY `idx_sort` (`sort`),
  KEY `idx_archive_count` (`archive_count`)
) ENGINE=InnoDB AUTO_INCREMENT=17 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `categories`
--

LOCK TABLES `categories` WRITE;
/*!40000 ALTER TABLE `categories` DISABLE KEYS */;
INSERT INTO `categories` (`id`, `created_time`, `updated_time`, `title`, `seo_title`, `keywords`, `url_token`, `description`, `content`, `module_id`, `parent_id`, `type`, `sort`, `template`, `detail_template`, `is_inherit`, `images`, `logo`, `extra`, `archive_count`, `status`) VALUES (1,1694683531,1794045126,'公司介绍','公司介绍','RustCMS,软件公司,企业介绍','about','RustCMS 软件有限公司成立于 20XX 年，总部位于中国某城市，是一家专注企业级软件研发与数字化服务的科技公司。','<p>RustCMS 软件有限公司成立于 20XX 年，总部位于中国某城市，是一家专注企业级软件研发与数字化服务的科技公司。我们围绕企业管理、协同办公、数据分析与行业数字化，为客户提供从咨询规划到系统实施、运维保障的一体化服务。</p><p>公司核心团队来自制造业信息化、互联网和咨询行业，既懂业务流程，也懂技术实现。我们坚持产品化与定制化相结合：标准产品覆盖通用场景，低代码平台支撑个性化需求，帮助客户以更低成本完成数字化落地。</p><p>目前，公司已服务制造、零售、医疗、教育、建筑等多个行业的数百家客户，形成了 ERP、OA、BI、MES、CRM 等完整的产品矩阵，并通过 ISO 9001、ISO 27001 和 CMMI 3 级认证。</p><p>我们相信，软件的价值在于让业务更简单。未来，公司将持续投入研发，与生态伙伴一起，为客户创造长期价值。</p>',0,0,3,99,'','',0,'[\"/uploads/software/theme-01.png\"]','/uploads/software/theme-04.png','\"null\"',0,1),(2,1694683773,1694785479,'联系我们','联系我们','联系方式,售前咨询,技术支持','contact','售前咨询、技术支持与商务合作，欢迎通过以下方式联系我们。','<p>感谢您对 RustCMS 的关注。无论是产品咨询、方案交流还是技术支持，我们都欢迎您随时联系。</p><p>售前咨询：如需了解产品功能、报价与实施方案，请拨打服务热线或在线留言，我们将在工作时间内尽快回复。</p><p>技术支持：已签约客户可通过客户服务平台提交工单，或联系专属客户成功经理，获得 7×12 小时技术支持服务。</p><p>商务合作：我们欢迎渠道伙伴、实施伙伴与生态厂商加入合作体系，共同拓展区域与行业市场。</p><p>公司地址：中国某城市高新区软件园；邮箱：contact@rustcms.com；服务热线：400-000-0000。</p>',1,0,3,99,'','',0,'{\"/uploads/software/theme-02.png\"}','','null',0,1),(3,1694684063,1694829213,'企业管理软件','企业管理软件','ERP,企业管理,进销存,供应链','erp','覆盖财务、供应链、生产的一体化管理软件。','',2,0,1,99,'','',0,'{}','','null',16,1),(4,1694684173,1694829281,'协同办公','协同办公','OA,协同办公,工作流,文档','oa','OA、工作流、文档与会议协同产品。','',2,0,1,99,'','',0,'{}','','null',5,1),(5,1694684173,1694829311,'数据分析','数据分析','BI,报表,数据集成,可视化','bi','BI、报表、数据集成与可视化产品。','',2,0,1,99,'','',0,'{}','','null',5,1),(6,1694684173,1694829355,'定制开发','定制开发','定制开发,系统集成,小程序,迁移','custom','门户、小程序、系统集成与遗留系统重构服务。','',2,0,1,99,'','',0,'{}','','null',5,1),(7,1694684292,1694684292,'行业方案','行业方案','行业方案,制造业,零售,医疗,教育','industry','面向制造、零售、医疗、教育行业的软件方案。','',2,3,1,99,'','',0,NULL,'','null',4,1),(8,1694684292,1694684292,'制造执行 MES','制造执行 MES','MES,生产执行,仓储,质量,排产','mes','生产执行、仓储、质量与排产管理。','',2,3,1,99,'','',0,NULL,'','null',4,1),(9,1694684292,1694684292,'客户关系 CRM','客户关系 CRM','CRM,客户管理,商机,营销自动化','crm','客户、商机与营销自动化管理。','',2,3,1,99,'','',0,NULL,'','null',2,1),(10,1694684292,1694684292,'人力资源 HR','人力资源 HR','HR,人事,薪酬,绩效','hr','组织、人事、薪酬与绩效管理。','',2,3,1,99,'','',0,NULL,'','null',2,1),(11,1694684292,1694684292,'财务共享','财务共享','财务共享,费控,报销,对账','fin','费控、报销与财务共享服务。','',2,3,1,99,'','',0,NULL,'','null',2,1),(12,1694684371,1794045115,'解决方案','解决方案','数字化转型,解决方案,数据中台','solution','面向制造、零售、医疗、教育等行业，提供从咨询规划到系统落地的数字化转型方案。','<p>我们围绕企业数字化的典型场景，提供三类解决方案：</p><p>一、管理数字化：以 ERP 为核心，打通财务、供应链、生产与销售数据，建立统一的经营管理视图。</p><p>二、协同数字化：以 OA 和工作流为入口，连接组织、流程与文档，让审批、协作和知识沉淀在线完成。</p><p>三、数据智能化：以数据中台和 BI 为支撑，完成数据采集、治理、建模与可视化，辅助经营决策。</p><p>每个项目都从业务调研和蓝图设计开始，结合客户现状制定分阶段实施路线，并提供培训、运维和持续优化服务，确保系统真正用起来、见效果。</p>',1,0,1,99,'article/list-solution.html','',1,'[\"/uploads/software/theme-03.png\"]','','\"null\"',6,1),(13,1694684371,1694829163,'新闻资讯','新闻资讯','公司新闻,产品动态,软件行业','news','公司动态、产品发布与行业资讯。','',1,0,1,99,'','',0,'{}','','null',16,1),(14,1694684371,1694830198,'技术分享','技术分享','技术分享,架构,性能,数据治理','tech','架构设计、性能优化与数据治理实践。','',1,0,1,99,'article/list-faq.html','',1,'{\"/uploads/202309/15/2a42f2ae38c5ae4b.webp\"}','','null',6,1),(15,1694684399,1694785794,'客户案例','客户案例','客户案例,实施案例,数字化转型','case','各行业客户的软件实施与数字化实践。','',3,0,1,99,'','',0,'{\"/uploads/202309/15/2a42f2ae38c5ae4b.webp\"}','','null',9,1);
/*!40000 ALTER TABLE `categories` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `comment_praises`
--

DROP TABLE IF EXISTS `comment_praises`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `comment_praises` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `comment_id` bigint(20) NOT NULL DEFAULT '0',
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `rate` tinyint(1) NOT NULL DEFAULT '1',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_comment_praises_archive_id` (`archive_id`),
  KEY `idx_comment_id` (`comment_id`),
  KEY `idx_user_comment_id` (`user_id`,`comment_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `comment_praises`
--

LOCK TABLES `comment_praises` WRITE;
/*!40000 ALTER TABLE `comment_praises` DISABLE KEYS */;
/*!40000 ALTER TABLE `comment_praises` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `comments`
--

DROP TABLE IF EXISTS `comments`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `comments` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `archive_id` bigint(20) NOT NULL DEFAULT '0',
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `user_name` varchar(32) NOT NULL DEFAULT '',
  `email` varchar(100) NOT NULL DEFAULT '',
  `ip` varchar(32) NOT NULL DEFAULT '',
  `vote_count` int(10) NOT NULL DEFAULT '0',
  `content` longtext,
  `parent_id` int(10) unsigned NOT NULL DEFAULT '0',
  `to_uid` int(10) unsigned NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `rating` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_archive_id` (`archive_id`),
  KEY `idx_user_id` (`user_id`),
  KEY `idx_vote_count` (`vote_count`),
  KEY `idx_parent_id` (`parent_id`),
  KEY `idx_to_uid` (`to_uid`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `comments`
--

LOCK TABLES `comments` WRITE;
/*!40000 ALTER TABLE `comments` DISABLE KEYS */;
INSERT INTO `comments` (`id`, `created_time`, `updated_time`, `archive_id`, `user_id`, `user_name`, `email`, `ip`, `vote_count`, `content`, `parent_id`, `to_uid`, `status`, `rating`) VALUES (1,1792642195,1792642195,38,2,'buyer01','b@example.com','127.0.0.1',0,'商品很好用，发货也快',0,0,1,5);
/*!40000 ALTER TABLE `comments` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `commissions`
--

DROP TABLE IF EXISTS `commissions`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `commissions` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `order_id` varchar(32) NOT NULL DEFAULT '',
  `order_amount` bigint(20) NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `withdraw_id` int(10) NOT NULL DEFAULT '0',
  `remark` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_commissions_user_id` (`user_id`),
  KEY `idx_commissions_order_id` (`order_id`),
  KEY `idx_withdraw_id` (`withdraw_id`)
) ENGINE=InnoDB AUTO_INCREMENT=11 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `commissions`
--

LOCK TABLES `commissions` WRITE;
/*!40000 ALTER TABLE `commissions` DISABLE KEYS */;
INSERT INTO `commissions` (`id`, `created_time`, `updated_time`, `user_id`, `order_id`, `order_amount`, `amount`, `status`, `withdraw_id`, `remark`) VALUES (3,1793666736,1793666736,1,'rustcms-commission-1793666736',10000,100,1,1,'佣金测试'),(4,1793666800,1793666800,1,'rustcms-commission-1793666800',10000,100,1,1,'佣金测试'),(5,1793666811,1793666811,1,'rustcms-commission-1793666811',10000,100,1,1,'佣金测试'),(7,1793670162,1793670162,1,'rustcms-wd-1793670162a',1000,100,1,1,''),(8,1793670162,1793670162,1,'rustcms-wd-1793670162b',1000,200,1,1,''),(9,1793670263,1793670263,1,'rustcms-wd-1793670263a',1000,100,1,1,''),(10,1793670263,1793670263,1,'rustcms-wd-1793670263b',1000,200,1,1,'');
/*!40000 ALTER TABLE `commissions` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `diy_pages`
--

DROP TABLE IF EXISTS `diy_pages`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `diy_pages` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `key` varchar(50) NOT NULL DEFAULT '',
  `title` varchar(100) NOT NULL DEFAULT '',
  `modules` longtext,
  `status` tinyint(1) NOT NULL DEFAULT '1',
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_diy_key` (`key`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `diy_pages`
--

LOCK TABLES `diy_pages` WRITE;
/*!40000 ALTER TABLE `diy_pages` DISABLE KEYS */;
INSERT INTO `diy_pages` (`id`, `created_time`, `updated_time`, `key`, `title`, `modules`, `status`) VALUES (1,1792654719,1794132878,'home','网站首页','[]',1);
/*!40000 ALTER TABLE `diy_pages` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `finances`
--

DROP TABLE IF EXISTS `finances`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `finances` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `direction` tinyint(1) NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '''金额''',
  `after_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '''变更后用户金额''',
  `action` tinyint(1) NOT NULL DEFAULT '0' COMMENT '''资金类型，1出售，2购买，3退款，4充值，5提现，6推广，7返现,8佣金''',
  `order_id` varchar(32) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `remark` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_finances_user_id` (`user_id`),
  KEY `idx_finances_order_id` (`order_id`)
) ENGINE=InnoDB AUTO_INCREMENT=9 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `finances`
--

LOCK TABLES `finances` WRITE;
/*!40000 ALTER TABLE `finances` DISABLE KEYS */;
INSERT INTO `finances` (`id`, `created_time`, `updated_time`, `user_id`, `direction`, `amount`, `after_amount`, `action`, `order_id`, `status`, `remark`) VALUES (1,1792642134,1792642134,1,0,5000,95000,9,'',0,'缴纳店铺保证金'),(2,1792642134,1792642134,1,1,2000,97000,9,'',0,'退还店铺保证金'),(4,1793666737,1793666737,1,2,1000,97000,2,'rustcms-order-1793666737',1,'购买'),(6,1793666800,1793666800,1,2,1000,97000,2,'rustcms-order-1793666800',1,'购买'),(7,1793666838,1793666838,1,2,1000,97000,2,'rustcms-order-1793666838',1,'购买');
/*!40000 ALTER TABLE `finances` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `guestbooks`
--

DROP TABLE IF EXISTS `guestbooks`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `guestbooks` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_name` varchar(250) NOT NULL DEFAULT '',
  `contact` varchar(250) NOT NULL DEFAULT '',
  `content` text,
  `ip` varchar(32) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `refer` varchar(250) NOT NULL DEFAULT '',
  `site_id` int(10) unsigned NOT NULL DEFAULT '0',
  `extra_data` longtext,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_gb_user_id` (`user_id`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `guestbooks`
--

LOCK TABLES `guestbooks` WRITE;
/*!40000 ALTER TABLE `guestbooks` DISABLE KEYS */;
/*!40000 ALTER TABLE `guestbooks` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `html_push_logs`
--

DROP TABLE IF EXISTS `html_push_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `html_push_logs` (
  `local_file` varchar(190) NOT NULL,
  `remote_file` varchar(190) DEFAULT NULL,
  `created_time` bigint(20) DEFAULT NULL,
  `mod_time` bigint(20) DEFAULT NULL,
  `status` tinyint(1) DEFAULT NULL,
  `error_msg` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`local_file`),
  KEY `idx_html_push_logs_created_time` (`created_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `html_push_logs`
--

LOCK TABLES `html_push_logs` WRITE;
/*!40000 ALTER TABLE `html_push_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `html_push_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `keywords`
--

DROP TABLE IF EXISTS `keywords`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `keywords` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(190) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `level` int(10) DEFAULT '0',
  `article_count` int(10) DEFAULT '0',
  `has_dig` tinyint(1) DEFAULT '0',
  `last_time` bigint(20) DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_keywords_title` (`title`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_status` (`status`),
  KEY `idx_category_id` (`category_id`),
  KEY `idx_keywords_article_count` (`article_count`),
  KEY `idx_keywords_last_time` (`last_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `keywords`
--

LOCK TABLES `keywords` WRITE;
/*!40000 ALTER TABLE `keywords` DISABLE KEYS */;
/*!40000 ALTER TABLE `keywords` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `links`
--

DROP TABLE IF EXISTS `links`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `links` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `link` varchar(250) NOT NULL DEFAULT '',
  `back_link` varchar(250) NOT NULL DEFAULT '',
  `my_title` varchar(250) NOT NULL DEFAULT '',
  `my_link` varchar(250) NOT NULL DEFAULT '',
  `contact` varchar(250) NOT NULL DEFAULT '',
  `remark` varchar(250) NOT NULL DEFAULT '',
  `nofollow` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `sort` int(10) unsigned NOT NULL DEFAULT '99',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `checked_time` bigint(20) NOT NULL DEFAULT '0',
  `logo` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_sort` (`sort`),
  KEY `idx_status` (`status`)
) ENGINE=InnoDB AUTO_INCREMENT=5 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `links`
--

LOCK TABLES `links` WRITE;
/*!40000 ALTER TABLE `links` DISABLE KEYS */;
INSERT INTO `links` (`id`, `created_time`, `updated_time`, `title`, `link`, `back_link`, `my_title`, `my_link`, `contact`, `remark`, `nofollow`, `sort`, `status`, `checked_time`, `logo`) VALUES (1,1794034701,1794034701,'中国软件行业协会','https://www.csia.org.cn','','','','','',0,99,1,0,''),(2,1794034701,1794034701,'中国电子信息行业联合会','https://www.cie.org.cn','','','','','',0,99,1,0,''),(3,1794034701,1794034701,'开源中国','https://www.oschina.net','','','','','',0,99,1,0,''),(4,1794132879,1794132879,'rustcms-route-link','https://example.com','','','','','',0,99,4,1794132880,'');
/*!40000 ALTER TABLE `links` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `mail_plans`
--

DROP TABLE IF EXISTS `mail_plans`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `mail_plans` (
  `id` bigint(20) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` int(11) DEFAULT NULL,
  `updated_time` int(11) DEFAULT NULL,
  `identifier` varchar(100) DEFAULT NULL COMMENT '邮件计划标识',
  `recipients` text COMMENT '收件人列表',
  `subject` varchar(255) NOT NULL DEFAULT '' COMMENT '邮件主题',
  `content` text COMMENT '邮件内容',
  `attach_path` varchar(255) DEFAULT NULL COMMENT '附件路径',
  `send_time` int(11) NOT NULL DEFAULT '0' COMMENT '计划发送时间戳',
  `executed_time` int(11) DEFAULT '0' COMMENT '实际执行时间',
  `status` tinyint(4) NOT NULL DEFAULT '0' COMMENT '状态0待发送1发送中2已发送-1已取消3发送错误',
  `send_result` varchar(1000) DEFAULT NULL COMMENT '发送结果统计',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_identifier` (`identifier`),
  KEY `idx_mail_plans_identifier` (`identifier`),
  KEY `idx_status_send_time` (`status`,`send_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `mail_plans`
--

LOCK TABLES `mail_plans` WRITE;
/*!40000 ALTER TABLE `mail_plans` DISABLE KEYS */;
/*!40000 ALTER TABLE `mail_plans` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `material_categories`
--

DROP TABLE IF EXISTS `material_categories`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `material_categories` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `material_count` int(10) unsigned NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `material_categories`
--

LOCK TABLES `material_categories` WRITE;
/*!40000 ALTER TABLE `material_categories` DISABLE KEYS */;
/*!40000 ALTER TABLE `material_categories` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `material_data`
--

DROP TABLE IF EXISTS `material_data`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `material_data` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `material_id` int(10) NOT NULL DEFAULT '0',
  `item_type` varchar(32) NOT NULL DEFAULT '',
  `item_id` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_material_data_material_id` (`material_id`),
  KEY `idx_item_type` (`item_type`,`item_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `material_data`
--

LOCK TABLES `material_data` WRITE;
/*!40000 ALTER TABLE `material_data` DISABLE KEYS */;
/*!40000 ALTER TABLE `material_data` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `materials`
--

DROP TABLE IF EXISTS `materials`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `materials` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `content` longtext,
  `origin_url` varchar(190) NOT NULL DEFAULT '',
  `keyword` varchar(250) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `auto_update` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `use_count` int(10) unsigned NOT NULL DEFAULT '0',
  `md5` varchar(32) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_category_id` (`category_id`),
  KEY `idx_materials_origin_url` (`origin_url`),
  KEY `idx_md5` (`md5`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `materials`
--

LOCK TABLES `materials` WRITE;
/*!40000 ALTER TABLE `materials` DISABLE KEYS */;
/*!40000 ALTER TABLE `materials` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `modules`
--

DROP TABLE IF EXISTS `modules`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `modules` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `table_name` varchar(50) NOT NULL DEFAULT '',
  `name` varchar(50) NOT NULL DEFAULT '',
  `url_token` varchar(50) NOT NULL DEFAULT '',
  `title` varchar(250) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `fields` longtext,
  `category_fields` longtext,
  `is_system` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `modules`
--

LOCK TABLES `modules` WRITE;
/*!40000 ALTER TABLE `modules` DISABLE KEYS */;
INSERT INTO `modules` (`id`, `created_time`, `updated_time`, `table_name`, `name`, `url_token`, `title`, `keywords`, `description`, `fields`, `category_fields`, `is_system`, `status`) VALUES (1,1791860113,1792637220,'article','资讯模块','news','资讯中心','','','null','null',1,1),(2,1791860113,1792637220,'product','产品模块','products','软件产品','','','null','null',1,1),(3,1694683908,1731119306,'case','案例模块','cases','客户案例','','','[]','null',0,1);
/*!40000 ALTER TABLE `modules` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `nav_types`
--

DROP TABLE IF EXISTS `nav_types`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `nav_types` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `nav_types`
--

LOCK TABLES `nav_types` WRITE;
/*!40000 ALTER TABLE `nav_types` DISABLE KEYS */;
INSERT INTO `nav_types` (`id`, `created_time`, `updated_time`, `title`) VALUES (1,1791860113,1792637223,'默认导航'),(2,1694746617,1694746617,'顶部导航');
/*!40000 ALTER TABLE `nav_types` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `navs`
--

DROP TABLE IF EXISTS `navs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `navs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `sub_title` varchar(250) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `parent_id` int(10) unsigned NOT NULL DEFAULT '0',
  `nav_type` int(10) unsigned NOT NULL DEFAULT '0',
  `page_id` bigint(20) NOT NULL DEFAULT '0',
  `type_id` int(10) unsigned NOT NULL DEFAULT '1',
  `link` varchar(250) NOT NULL DEFAULT '',
  `logo` varchar(250) NOT NULL DEFAULT '',
  `style` varchar(250) NOT NULL DEFAULT '',
  `sort` int(10) unsigned NOT NULL DEFAULT '99',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_parent_id` (`parent_id`),
  KEY `idx_nav_type` (`nav_type`),
  KEY `idx_page_id` (`page_id`),
  KEY `idx_type_id` (`type_id`),
  KEY `idx_sort` (`sort`)
) ENGINE=InnoDB AUTO_INCREMENT=17 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `navs`
--

LOCK TABLES `navs` WRITE;
/*!40000 ALTER TABLE `navs` DISABLE KEYS */;
INSERT INTO `navs` (`id`, `created_time`, `updated_time`, `title`, `sub_title`, `description`, `parent_id`, `nav_type`, `page_id`, `type_id`, `link`, `logo`, `style`, `sort`, `status`) VALUES (1,1694683219,1694683219,'首页','','',0,0,0,1,'','','',99,1),(2,1694746397,1694746397,'公司介绍','','',0,1,1,1,'','','',99,1),(3,1694746442,1694746442,'产品中心','','',0,0,2,1,'','','',99,1),(4,1694746479,1694746479,'企业管理软件','','',3,1,3,1,'','','',99,1),(5,1694746504,1694746504,'协同办公','','',3,1,4,1,'','','',99,1),(6,1694746515,1694746515,'数据分析','','',3,1,5,1,'','','',99,1),(7,1694746532,1694746532,'定制开发','','',3,1,6,1,'','','',99,1),(8,1694746549,1694746549,'解决方案','','',0,1,12,1,'','','',99,1),(9,1694746559,1694746559,'客户案例','','',0,1,15,1,'','','',99,1),(10,1694746573,1694746573,'新闻资讯','','',0,1,13,1,'','','',99,1),(12,1694746590,1694746590,'联系我们','','',0,1,2,1,'','','',99,1),(13,1694746635,1694746635,'公司介绍','','',0,1,1,2,'','','',99,1),(14,1694746645,1694746645,'联系我们','','',0,1,2,2,'','','',99,1),(15,1694746669,1694746669,'网站地图','','',0,2,0,2,'/sitemap.xml','','',99,1);
/*!40000 ALTER TABLE `navs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `order_addresses`
--

DROP TABLE IF EXISTS `order_addresses`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `order_addresses` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `name` varchar(64) NOT NULL DEFAULT '',
  `last_name` varchar(64) NOT NULL DEFAULT '',
  `phone` varchar(20) NOT NULL DEFAULT '',
  `email` varchar(100) NOT NULL DEFAULT '',
  `province` varchar(100) NOT NULL DEFAULT '',
  `province_code` varchar(100) NOT NULL DEFAULT '',
  `city` varchar(100) NOT NULL DEFAULT '',
  `town` varchar(100) NOT NULL DEFAULT '',
  `country` varchar(100) NOT NULL DEFAULT '',
  `country_code` varchar(100) NOT NULL DEFAULT '',
  `address_info` varchar(255) NOT NULL DEFAULT '',
  `company` varchar(250) NOT NULL DEFAULT '',
  `postcode` varchar(36) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_order_addresses_user_id` (`user_id`),
  KEY `idx_order_addresses_phone` (`phone`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `order_addresses`
--

LOCK TABLES `order_addresses` WRITE;
/*!40000 ALTER TABLE `order_addresses` DISABLE KEYS */;
/*!40000 ALTER TABLE `order_addresses` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `order_details`
--

DROP TABLE IF EXISTS `order_details`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `order_details` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `order_id` varchar(36) NOT NULL DEFAULT '',
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `goods_id` bigint(20) NOT NULL,
  `goods_sku_id` bigint(20) NOT NULL DEFAULT '0',
  `price` bigint(20) NOT NULL DEFAULT '0',
  `origin_price` bigint(20) NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0',
  `origin_amount` bigint(20) NOT NULL DEFAULT '0',
  `quantity` int(10) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_order_details_order_id` (`order_id`),
  KEY `idx_order_details_user_id` (`user_id`),
  KEY `idx_order_details_goods_id` (`goods_id`),
  KEY `idx_order_details_goods_sku_id` (`goods_sku_id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `order_details`
--

LOCK TABLES `order_details` WRITE;
/*!40000 ALTER TABLE `order_details` DISABLE KEYS */;
INSERT INTO `order_details` (`id`, `created_time`, `updated_time`, `order_id`, `user_id`, `goods_id`, `goods_sku_id`, `price`, `origin_price`, `amount`, `origin_amount`, `quantity`, `status`) VALUES (2,1792642307,1792642307,'202610221211470003',1,38,0,19900,19900,59700,59700,3,1);
/*!40000 ALTER TABLE `order_details` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `order_refunds`
--

DROP TABLE IF EXISTS `order_refunds`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `order_refunds` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `refund_id` varchar(36) NOT NULL,
  `order_id` varchar(36) NOT NULL,
  `detail_id` int(10) NOT NULL DEFAULT '0',
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '''退款金额''',
  `refund_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '支付时间',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `error_times` int(10) NOT NULL DEFAULT '0',
  `last_time` bigint(20) NOT NULL DEFAULT '0',
  `remark` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_order_refunds_refund_id` (`refund_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_order_refunds_user_id` (`user_id`),
  KEY `idx_order_refunds_amount` (`amount`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `order_refunds`
--

LOCK TABLES `order_refunds` WRITE;
/*!40000 ALTER TABLE `order_refunds` DISABLE KEYS */;
/*!40000 ALTER TABLE `order_refunds` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `orders`
--

DROP TABLE IF EXISTS `orders`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `orders` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `order_id` varchar(36) NOT NULL,
  `payment_id` varchar(36) NOT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `address_id` int(10) unsigned NOT NULL DEFAULT '0',
  `remark` varchar(250) NOT NULL DEFAULT '',
  `type` varchar(20) NOT NULL DEFAULT 'goods',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `refund_status` tinyint(1) NOT NULL DEFAULT '0',
  `origin_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '订单商品总价',
  `amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '订单计算运费、优惠后的总价',
  `paid_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '支付时间',
  `end_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '在确认支付之前，为订单关闭时间，在支付完成后，为自动确认收货时间',
  `deliver_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '发货时间',
  `finished_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '订单完成时间',
  `discount_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '优惠金额',
  `user_discount` bigint(20) NOT NULL DEFAULT '0' COMMENT '用户优惠金额',
  `coupon_discount` bigint(20) NOT NULL DEFAULT '0' COMMENT '优惠券优惠金额',
  `coupon_code_id` varchar(36) NOT NULL DEFAULT '',
  `seller_id` int(10) unsigned NOT NULL DEFAULT '0',
  `seller_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '卖家可得金额',
  `share_user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `share_parent_user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `share_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '分销可得金额',
  `share_parent_amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '奖励金，上级',
  `express_company` varchar(100) NOT NULL DEFAULT '',
  `tracking_number` varchar(100) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_orders_order_id` (`order_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_orders_payment_id` (`payment_id`),
  KEY `idx_orders_user_id` (`user_id`),
  KEY `idx_orders_seller_id` (`seller_id`),
  KEY `idx_orders_share_user_id` (`share_user_id`),
  KEY `idx_orders_share_parent_user_id` (`share_parent_user_id`)
) ENGINE=InnoDB AUTO_INCREMENT=14 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `orders`
--

LOCK TABLES `orders` WRITE;
/*!40000 ALTER TABLE `orders` DISABLE KEYS */;
INSERT INTO `orders` (`id`, `created_time`, `updated_time`, `order_id`, `payment_id`, `user_id`, `address_id`, `remark`, `type`, `status`, `refund_status`, `origin_amount`, `amount`, `paid_time`, `end_time`, `deliver_time`, `finished_time`, `discount_amount`, `user_discount`, `coupon_discount`, `coupon_code_id`, `seller_id`, `seller_amount`, `share_user_id`, `share_parent_user_id`, `share_amount`, `share_parent_amount`, `express_company`, `tracking_number`) VALUES (1,1792640711,1792640711,'TEST20261022001','',1,0,'','goods',1,0,19900,19900,0,0,0,0,0,0,0,'',0,0,0,0,0,0,'',''),(2,1792642195,1792642195,'SALESTEST001','',2,0,'','goods',1,0,12000,12000,0,0,0,0,0,0,0,'',1,10800,0,0,0,0,'',''),(3,1792642307,1792642307,'202610221211470003','',1,0,'test','goods',0,0,59700,59700,0,0,0,0,0,0,0,'',0,0,0,0,0,0,'',''),(7,1793666737,1793762221,'rustcms-order-1793666737','AUnQmgJCKKP4Pfz13LcCszDh5yfni0i5',1,0,'','goods',3,0,0,1000,1793666737,0,0,1793762221,0,0,0,'',0,0,0,0,0,0,'',''),(10,1793666800,1793762221,'rustcms-order-1793666800','9s0g5XsMdZth1Cg13TNG8IzBiKcoqPvP',1,0,'','goods',3,0,0,1000,1793666800,0,0,1793762221,0,0,0,'',0,0,0,0,0,0,'',''),(11,1793666838,1793762221,'rustcms-order-1793666838','H5D31wq9RjA1mduNN4jqT5R5wbnvdjMX',1,0,'','goods',3,0,0,1000,1793666838,0,0,1793762221,0,0,0,'',0,0,0,0,0,0,'','');
/*!40000 ALTER TABLE `orders` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `payment_accounts`
--

DROP TABLE IF EXISTS `payment_accounts`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `payment_accounts` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` int(11) DEFAULT NULL,
  `updated_time` int(11) DEFAULT NULL,
  `pay_way` varchar(32) NOT NULL DEFAULT '',
  `name` varchar(100) NOT NULL,
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `health_score` int(10) NOT NULL DEFAULT '0',
  `pay_config` text,
  `min_amount` bigint(20) NOT NULL DEFAULT '0',
  `max_amount` bigint(20) NOT NULL DEFAULT '0',
  `daily_amount_limit` bigint(20) NOT NULL DEFAULT '0',
  `daily_count_limit` bigint(20) NOT NULL DEFAULT '0',
  `monthly_amount_limit` bigint(20) NOT NULL DEFAULT '0',
  `last_used_time` int(11) NOT NULL DEFAULT '0',
  `used_count` bigint(20) NOT NULL DEFAULT '0',
  `used_amount` bigint(20) NOT NULL DEFAULT '0',
  `weight` bigint(20) NOT NULL DEFAULT '0',
  `is_default` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_payment_accounts_pay_way` (`pay_way`)
) ENGINE=InnoDB AUTO_INCREMENT=9 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `payment_accounts`
--

LOCK TABLES `payment_accounts` WRITE;
/*!40000 ALTER TABLE `payment_accounts` DISABLE KEYS */;
INSERT INTO `payment_accounts` (`id`, `created_time`, `updated_time`, `pay_way`, `name`, `status`, `health_score`, `pay_config`, `min_amount`, `max_amount`, `daily_amount_limit`, `daily_count_limit`, `monthly_amount_limit`, `last_used_time`, `used_count`, `used_amount`, `weight`, `is_default`) VALUES (1,1793670157,1793670157,'wechat','测试账户',1,90,'{\"app_id\":\"wx123\"}',100,100000,50000,10,1000000,0,0,0,5,1),(2,1793670213,1793670213,'alipay','路由测试账户',1,0,'{\"app_id\":\"ali123\"}',100,0,0,0,0,0,0,0,0,0),(3,1793670263,1793670263,'wechat','测试账户',1,90,'{\"app_id\":\"wx123\"}',100,100000,50000,10,1000000,0,0,0,5,1),(4,1793670264,1793670264,'alipay','路由测试账户',1,0,'{\"app_id\":\"ali123\"}',100,0,0,0,0,0,0,0,0,0),(5,1793670534,1793670534,'wechat','测试账户',1,90,'{\"app_id\":\"wx123\"}',100,100000,50000,10,1000000,0,0,0,5,1),(7,1793670645,1793670645,'wechat','测试账户',1,90,'{\"app_id\":\"wx123\"}',100,100000,50000,10,1000000,0,0,0,5,1);
/*!40000 ALTER TABLE `payment_accounts` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `payment_statistics`
--

DROP TABLE IF EXISTS `payment_statistics`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `payment_statistics` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `account_id` int(10) unsigned NOT NULL DEFAULT '0',
  `stat_time` int(11) NOT NULL DEFAULT '0',
  `daily_amount` bigint(20) NOT NULL DEFAULT '0',
  `monthly_amount` bigint(20) NOT NULL DEFAULT '0',
  `daily_count` int(10) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_payment_statistics_account_id` (`account_id`),
  KEY `idx_payment_statistics_stat_time` (`stat_time`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `payment_statistics`
--

LOCK TABLES `payment_statistics` WRITE;
/*!40000 ALTER TABLE `payment_statistics` DISABLE KEYS */;
/*!40000 ALTER TABLE `payment_statistics` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `payments`
--

DROP TABLE IF EXISTS `payments`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `payments` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `payment_id` varchar(36) NOT NULL,
  `terrace_id` varchar(64) NOT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `order_id` varchar(36) NOT NULL,
  `amount` bigint(20) NOT NULL DEFAULT '0' COMMENT '''支付总价''',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `payment_account_id` int(10) NOT NULL DEFAULT '0',
  `pay_way` varchar(32) NOT NULL DEFAULT '',
  `paid_time` bigint(20) NOT NULL DEFAULT '0' COMMENT '支付时间',
  `remark` varchar(255) DEFAULT NULL,
  `buyer_id` varchar(64) NOT NULL DEFAULT '',
  `buyer_info` varchar(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_payments_payment_id` (`payment_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_payments_terrace_id` (`terrace_id`),
  KEY `idx_payments_user_id` (`user_id`),
  KEY `idx_payments_order_id` (`order_id`),
  KEY `idx_payments_pay_way` (`pay_way`)
) ENGINE=InnoDB AUTO_INCREMENT=7 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `payments`
--

LOCK TABLES `payments` WRITE;
/*!40000 ALTER TABLE `payments` DISABLE KEYS */;
INSERT INTO `payments` (`id`, `created_time`, `updated_time`, `payment_id`, `terrace_id`, `user_id`, `order_id`, `amount`, `status`, `payment_account_id`, `pay_way`, `paid_time`, `remark`, `buyer_id`, `buyer_info`) VALUES (2,1793666737,1793666737,'AUnQmgJCKKP4Pfz13LcCszDh5yfni0i5','1793666737',1,'rustcms-order-1793666737',1000,1,0,'offline',1793666737,NULL,'',''),(4,1793666800,1793666800,'9s0g5XsMdZth1Cg13TNG8IzBiKcoqPvP','1793666800',1,'rustcms-order-1793666800',1000,1,0,'offline',1793666800,NULL,'',''),(5,1793666838,1793666838,'H5D31wq9RjA1mduNN4jqT5R5wbnvdjMX','1793666838',1,'rustcms-order-1793666838',1000,1,0,'offline',1793666838,NULL,'','');
/*!40000 ALTER TABLE `payments` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `places`
--

DROP TABLE IF EXISTS `places`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `places` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `seo_title` varchar(250) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `content` longtext,
  `parent_id` int(10) unsigned NOT NULL DEFAULT '0',
  `sort` int(10) unsigned NOT NULL DEFAULT '99',
  `template` varchar(250) NOT NULL DEFAULT '',
  `is_inherit` int(1) unsigned NOT NULL DEFAULT '0',
  `images` text,
  `logo` varchar(250) NOT NULL DEFAULT '',
  `extra` longtext,
  `latitude` decimal(10,6) NOT NULL DEFAULT '0.000000',
  `longitude` decimal(10,6) NOT NULL DEFAULT '0.000000',
  `timezone` varchar(100) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_places_url_token` (`url_token`),
  KEY `idx_parent_id` (`parent_id`),
  KEY `idx_sort` (`sort`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `places`
--

LOCK TABLES `places` WRITE;
/*!40000 ALTER TABLE `places` DISABLE KEYS */;
/*!40000 ALTER TABLE `places` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `point_logs`
--

DROP TABLE IF EXISTS `point_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `point_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `points` bigint(20) NOT NULL DEFAULT '0',
  `after_points` bigint(20) NOT NULL DEFAULT '0',
  `type` tinyint(1) NOT NULL DEFAULT '1',
  `remark` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_point_user_id` (`user_id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `point_logs`
--

LOCK TABLES `point_logs` WRITE;
/*!40000 ALTER TABLE `point_logs` DISABLE KEYS */;
INSERT INTO `point_logs` (`id`, `created_time`, `updated_time`, `user_id`, `points`, `after_points`, `type`, `remark`) VALUES (1,1792642123,1792642123,1,5,5,1,'签到第 1 天奖励'),(2,1792647232,1792647232,2,5,5,1,'签到第 1 天奖励');
/*!40000 ALTER TABLE `point_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `product`
--

DROP TABLE IF EXISTS `product`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `product` (
  `id` bigint(20) NOT NULL AUTO_INCREMENT,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=74 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `product`
--

LOCK TABLES `product` WRITE;
/*!40000 ALTER TABLE `product` DISABLE KEYS */;
INSERT INTO `product` (`id`) VALUES (27),(28),(29),(30),(31),(32),(33),(34),(35),(36),(37),(38),(39),(40),(41),(42),(43),(44),(45),(46),(47),(48),(59),(60),(61),(68),(69),(70),(71),(72),(73);
/*!40000 ALTER TABLE `product` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `recharges`
--

DROP TABLE IF EXISTS `recharges`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `recharges` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0',
  `pay_way` varchar(20) NOT NULL DEFAULT '',
  `remark` varchar(250) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `confirm_time` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_recharge_user_id` (`user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `recharges`
--

LOCK TABLES `recharges` WRITE;
/*!40000 ALTER TABLE `recharges` DISABLE KEYS */;
/*!40000 ALTER TABLE `recharges` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `redirects`
--

DROP TABLE IF EXISTS `redirects`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `redirects` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `from_url` varchar(190) NOT NULL DEFAULT '',
  `to_url` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_redirects_from_url` (`from_url`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `redirects`
--

LOCK TABLES `redirects` WRITE;
/*!40000 ALTER TABLE `redirects` DISABLE KEYS */;
/*!40000 ALTER TABLE `redirects` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `settings`
--

DROP TABLE IF EXISTS `settings`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `settings` (
  `key` varchar(190) NOT NULL,
  `value` longtext,
  PRIMARY KEY (`key`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `settings`
--

LOCK TABLES `settings` WRITE;
/*!40000 ALTER TABLE `settings` DISABLE KEYS */;
INSERT INTO `settings` (`key`, `value`) VALUES ('akismet','{\"open\":true,\"api_key\":\"rustcms-key\"}'),('banner','{\"banner\":[]}'),('cache_type',''),('contact','{\"user_name\":\"\",\"cellphone\":\"\",\"address\":\"\",\"email\":\"\",\"wechat\":\"\",\"qrcode\":\"\",\"extra_fields\":[]}'),('content','{\"remote_download\":1,\"filter_outlink\":1,\"url_token_type\":0,\"use_webp\":1,\"quality\":90,\"resize_image\":1,\"resize_width\":1200,\"thumb_crop\":1,\"thumb_width\":300,\"thumb_height\":200,\"default_thumb\":\"\",\"multi_category\":0,\"editor\":\"\",\"default_thumb_type\":null,\"default_thumbs\":[],\"thumb_category_id\":null,\"use_sort\":null,\"max_page\":null,\"max_limit\":null}'),('diy_fields','[{\"name\":\"作者\",\"field_name\":\"author\",\"type\":\"text\"}]'),('fulltext','{\"open\":true,\"use_content\":false,\"modules\":[1,2],\"engine\":\"meilisearch\",\"ranking_score\":30,\"initialed\":true}'),('guestbook','{\"return_message\":\"\",\"fields\":[{\"name\":\"用户名\",\"field_name\":\"user_name\",\"type\":\"text\",\"required\":true,\"is_system\":true,\"is_filter\":false,\"follow_level\":false,\"content\":\"\"},{\"name\":\"联系电话\",\"field_name\":\"contact\",\"type\":\"text\",\"required\":true,\"is_system\":true,\"is_filter\":false,\"follow_level\":false,\"content\":\"\"},{\"name\":\"Email\",\"field_name\":\"email\",\"type\":\"text\",\"required\":false,\"is_system\":false,\"is_filter\":false,\"follow_level\":false,\"content\":\"\"},{\"name\":\"留言内容\",\"field_name\":\"content\",\"type\":\"textarea\",\"required\":false,\"is_system\":true,\"is_filter\":false,\"follow_level\":false,\"content\":\"\"}]}'),('import_api','{\"token\":\"66f76d6979428d371bb811009539382f\",\"link_token\":\"66f76d6979428d371bb811009539382f\"}'),('index','{\"seo_title\":\"Rust 首页标题\",\"seo_keywords\":\"RustCMS,企业管理软件,低代码,数据中台,协同办公\",\"seo_description\":\"RustCMS 软件有限公司专注企业级软件研发，提供 ERP、OA、数据分析和低代码平台，服务制造、零售、医疗等行业客户。\"}'),('install_time','1792637224'),('interference','{\"open\":false,\"mode\":0,\"disable_selection\":false,\"disable_copy\":false,\"disable_right_click\":false}'),('last_run_version','3.6.8'),('push','{\"baidu_api\":\"\",\"bing_api\":\"\",\"js_code\":\"\",\"js_codes\":[]}'),('rewrite','{\"mode\":0,\"patten\":\"\"}'),('safe','{\"captcha\":0,\"daily_limit\":5,\"content_limit\":0,\"interval_limit\":360,\"content_forbidden\":\"\",\"ip_forbidden\":\"\",\"ua_forbidden\":\"\",\"api_open\":0,\"api_publish\":0,\"admin_captcha_off\":1}'),('sendmail','{\"server\":\"smtp.example.com\",\"use_ssl\":1,\"port\":587,\"account\":\"bot@example.com\",\"password\":\"real-secret-16char\",\"recipient\":\"admin@example.com\",\"templates\":[{\"open\":true,\"key\":\"guestbook\",\"delay\":30,\"subject\":\"自定义标题\",\"content\":\"自定义内容\"},{\"open\":true,\"key\":\"report\",\"delay\":0,\"subject\":\"hack\",\"content\":\"hack\"}]}'),('sensitive_words','[\"第一\",\"第二|很好\",\"{\\\\d{6,12}}\"]'),('sitemap','{\"auto_build\":1,\"type\":\"xml\",\"updated_time\":0,\"sitemap_url\":\"\"}'),('storage','{\"storage_url\":\"https://cdn.example.com/\",\"storage_type\":\"local\",\"keep_local\":true,\"aliyun_endpoint\":\"\",\"aliyun_access_key_id\":\"\",\"aliyun_access_key_secret\":\"\",\"aliyun_bucket_name\":\"\",\"tencent_secret_id\":\"\",\"tencent_secret_key\":\"\",\"tencent_bucket_url\":\"\",\"qiniu_access_key\":\"\",\"qiniu_secret_key\":\"\",\"qiniu_bucket\":\"\",\"qiniu_region\":\"\",\"upyun_bucket\":\"\",\"upyun_operator\":\"\",\"upyun_password\":\"\",\"ftp_host\":\"\",\"ftp_port\":0,\"ftp_username\":\"\",\"ftp_password\":\"\",\"ftp_webroot\":\"/var/www\",\"ssh_host\":\"\",\"ssh_port\":0,\"ssh_username\":\"\",\"ssh_password\":\"\",\"ssh_private_key\":\"ssh_private_key.key\",\"ssh_webroot\":\"/data\",\"google_project_id\":\"\",\"google_credentials_json\":\"\",\"google_bucket_name\":\"\",\"s3_region\":\"\",\"s3_bucket\":\"\",\"s3_access_key\":\"\",\"s3_secret_key\":\"\",\"s3_endpoint\":\"\"}'),('system','{\"favicon\":\"\",\"base_url\":\"\",\"language\":\"zh\",\"site_icp\":\"\",\"admin_url\":\"\",\"front_url\":\"\",\"site_logo\":\"/uploads/software/logo.png\",\"site_name\":\"Rust 测试站点\",\"ban_spider\":0,\"mobile_url\":\"\",\"site_close\":0,\"default_site\":false,\"extra_fields\":[],\"template_url\":\"\",\"token_secret\":\"rWaomnHBIKJYNRjpKWPIm7yJk3tN1nl0\",\"template_name\":\"default\",\"template_type\":0,\"site_copyright\":\"\",\"site_close_tips\":\"\"}'),('upgrade_archive_category','\"2023-09-14 16:35:20\"'),('upload_file','[{\"hash\":\"b01c396121011d2be46df06a2069313c\",\"file_name\":\"rustcms-verify.xml\",\"created_time\":1793586325}]'),('watermark','{\"open\":true,\"size\":20,\"opacity\":100}'),('weapp','{\"app_id\":\"wxapp123\"}'),('wechat','{\"app_id\":\"wxmp456\",\"verify_key\":\"rustcms\"}');
/*!40000 ALTER TABLE `settings` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `shop_deposit_logs`
--

DROP TABLE IF EXISTS `shop_deposit_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `shop_deposit_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `shop_id` int(10) unsigned NOT NULL DEFAULT '0',
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0',
  `after_amount` bigint(20) NOT NULL DEFAULT '0',
  `type` tinyint(1) NOT NULL DEFAULT '1',
  `remark` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_deposit_shop_id` (`shop_id`),
  KEY `idx_deposit_user_id` (`user_id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `shop_deposit_logs`
--

LOCK TABLES `shop_deposit_logs` WRITE;
/*!40000 ALTER TABLE `shop_deposit_logs` DISABLE KEYS */;
INSERT INTO `shop_deposit_logs` (`id`, `created_time`, `updated_time`, `shop_id`, `user_id`, `amount`, `after_amount`, `type`, `remark`) VALUES (1,1792642134,1792642134,1,1,5000,5000,1,'缴纳店铺保证金'),(2,1792642134,1792642134,1,1,-2000,3000,2,'退还店铺保证金');
/*!40000 ALTER TABLE `shop_deposit_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `shops`
--

DROP TABLE IF EXISTS `shops`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `shops` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `name` varchar(60) NOT NULL DEFAULT '',
  `logo` varchar(255) NOT NULL DEFAULT '',
  `banner` varchar(255) NOT NULL DEFAULT '',
  `intro` varchar(1000) NOT NULL DEFAULT '',
  `notice` varchar(500) NOT NULL DEFAULT '',
  `contact` varchar(100) NOT NULL DEFAULT '',
  `theme_color` varchar(20) NOT NULL DEFAULT '',
  `decoration` text,
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `deposit` bigint(20) NOT NULL DEFAULT '0',
  `deposit_total` bigint(20) NOT NULL DEFAULT '0',
  `product_count` bigint(20) NOT NULL DEFAULT '0',
  `order_count` bigint(20) NOT NULL DEFAULT '0',
  `sale_amount` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_shop_user_id` (`user_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `shops`
--

LOCK TABLES `shops` WRITE;
/*!40000 ALTER TABLE `shops` DISABLE KEYS */;
INSERT INTO `shops` (`id`, `created_time`, `updated_time`, `user_id`, `name`, `logo`, `banner`, `intro`, `notice`, `contact`, `theme_color`, `decoration`, `status`, `deposit`, `deposit_total`, `product_count`, `order_count`, `sale_amount`) VALUES (1,1792642116,1792651582,1,'TestShop','','','','','','','{\"layout\":\"wide\",\"theme_color\":\"\",\"banner\":\"\",\"footer\":\"\",\"show_notice\":true,\"notice\":\"\",\"modules\":[\"products\",\"notice\",\"contact\"]}',1,3000,5000,0,0,0),(2,1792642728,1792642728,2,'yundayin','','','这个店铺还没有填写简介','','','','{\"layout\":\"wide\",\"banner\":\"\",\"show_notice\":true,\"notice\":\"\",\"modules\":[\"products\",\"notice\",\"contact\"],\"theme_color\":\"\",\"footer\":\"\"}',1,0,0,0,0,0);
/*!40000 ALTER TABLE `shops` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `spider_includes`
--

DROP TABLE IF EXISTS `spider_includes`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `spider_includes` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `baidu_count` int(10) DEFAULT '0',
  `sogou_count` int(10) DEFAULT '0',
  `so_count` int(10) DEFAULT '0',
  `bing_count` int(10) DEFAULT '0',
  `google_count` int(10) DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_spider_includes_created_time` (`created_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `spider_includes`
--

LOCK TABLES `spider_includes` WRITE;
/*!40000 ALTER TABLE `spider_includes` DISABLE KEYS */;
/*!40000 ALTER TABLE `spider_includes` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `statistic_logs`
--

DROP TABLE IF EXISTS `statistic_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `statistic_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `spider_count` varchar(250) DEFAULT NULL,
  `visit_count` varchar(250) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uni_statistic_logs_created_time` (`created_time`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `statistic_logs`
--

LOCK TABLES `statistic_logs` WRITE;
/*!40000 ALTER TABLE `statistic_logs` DISABLE KEYS */;
INSERT INTO `statistic_logs` (`id`, `created_time`, `spider_count`, `visit_count`) VALUES (1,1792598400,'{}','{\"ip_count\":1,\"pv_count\":748}'),(2,1793721600,'{}','{\"ip_count\":1,\"pv_count\":4}');
/*!40000 ALTER TABLE `statistic_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `subscriber_categories`
--

DROP TABLE IF EXISTS `subscriber_categories`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `subscriber_categories` (
  `id` int(11) NOT NULL AUTO_INCREMENT,
  `title` varchar(100) NOT NULL,
  `subscriber_count` int(11) DEFAULT NULL,
  `created_time` int(11) DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `subscriber_categories`
--

LOCK TABLES `subscriber_categories` WRITE;
/*!40000 ALTER TABLE `subscriber_categories` DISABLE KEYS */;
/*!40000 ALTER TABLE `subscriber_categories` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `subscribers`
--

DROP TABLE IF EXISTS `subscribers`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `subscribers` (
  `id` int(11) NOT NULL AUTO_INCREMENT,
  `user_id` int(11) DEFAULT NULL,
  `email` varchar(100) NOT NULL,
  `category_id` int(11) DEFAULT NULL,
  `remark` varchar(255) DEFAULT NULL,
  `status` tinyint(2) DEFAULT NULL,
  `send_times` int(11) DEFAULT NULL,
  `error_times` int(11) DEFAULT NULL,
  `created_time` int(11) DEFAULT NULL,
  `updated_time` int(11) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_subscribers_user_id` (`user_id`),
  KEY `idx_subscribers_email` (`email`),
  KEY `idx_subscribers_category_id` (`category_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `subscribers`
--

LOCK TABLES `subscribers` WRITE;
/*!40000 ALTER TABLE `subscribers` DISABLE KEYS */;
/*!40000 ALTER TABLE `subscribers` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `tag_contents`
--

DROP TABLE IF EXISTS `tag_contents`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `tag_contents` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `content` longtext,
  `extra` longtext,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=17 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `tag_contents`
--

LOCK TABLES `tag_contents` WRITE;
/*!40000 ALTER TABLE `tag_contents` DISABLE KEYS */;
INSERT INTO `tag_contents` (`id`, `content`, `extra`) VALUES (15,'','{}');
/*!40000 ALTER TABLE `tag_contents` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `tag_data`
--

DROP TABLE IF EXISTS `tag_data`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `tag_data` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `tag_id` int(10) NOT NULL DEFAULT '0',
  `item_id` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_tag_data_tag_id` (`tag_id`),
  KEY `idx_item_id` (`item_id`)
) ENGINE=InnoDB AUTO_INCREMENT=23 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `tag_data`
--

LOCK TABLES `tag_data` WRITE;
/*!40000 ALTER TABLE `tag_data` DISABLE KEYS */;
INSERT INTO `tag_data` (`id`, `tag_id`, `item_id`) VALUES (1,1,22),(2,2,22),(3,1,23),(4,3,23),(5,1,24),(6,4,24),(7,2,47),(8,1,47),(9,5,48),(10,2,48),(11,3,45),(12,2,45),(13,3,44),(14,2,44),(15,1,42),(16,1,40),(17,1,39),(18,1,41),(19,6,50),(20,7,50),(21,8,51),(22,9,51);
/*!40000 ALTER TABLE `tag_data` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `tags`
--

DROP TABLE IF EXISTS `tags`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `tags` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(250) NOT NULL DEFAULT '',
  `category_id` int(10) unsigned NOT NULL DEFAULT '0',
  `seo_title` varchar(250) NOT NULL DEFAULT '',
  `keywords` varchar(250) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `first_letter` char(1) NOT NULL DEFAULT '',
  `template` varchar(250) NOT NULL DEFAULT '',
  `logo` varchar(250) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_tags_category_id` (`category_id`),
  KEY `idx_tags_url_token` (`url_token`),
  KEY `idx_tags_first_letter` (`first_letter`),
  KEY `idx_status` (`status`)
) ENGINE=InnoDB AUTO_INCREMENT=17 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `tags`
--

LOCK TABLES `tags` WRITE;
/*!40000 ALTER TABLE `tags` DISABLE KEYS */;
INSERT INTO `tags` (`id`, `created_time`, `updated_time`, `title`, `category_id`, `seo_title`, `keywords`, `url_token`, `description`, `first_letter`, `template`, `logo`, `status`) VALUES (1,1694745939,1694745939,'ERP',0,'','','erp','','E','','',1),(2,1694745939,1694745939,'MES',0,'','','mes','','M','','',1),(3,1694745971,1694745971,'CRM',0,'','','crm','','C','','',1),(4,1694745992,1694745992,'WMS',0,'','','wms','','W','','',1),(5,1694746048,1694746048,'BI',0,'','','bi','','B','','',1),(6,1792643416,1792643416,'低代码',0,'','','low-code','','D','','',1),(7,1792643416,1792643416,'数据中台',0,'','','data-platform','','S','','',1),(8,1792643638,1792643638,'协同办公',0,'','','oa','','X','','',1),(9,1792643638,1792643638,'人力资源',0,'','','hr','','R','','',1),(10,1792646804,1792646804,'财务共享',0,'','','finance','','C','','',1),(11,1792646804,1792646804,'供应链协同',0,'','','supply-chain','','G','','',1),(12,1792646822,1792646822,'智能制造',0,'','','intelligent-manufacturing','','Z','','',1),(13,1792647007,1792647007,'数字化转型',0,'','','digital-transformation','','S','','',1),(14,1792647007,1792647007,'项目管理',0,'','','project-management','','X','','',1),(15,1792647322,1794045120,'客户关系',0,'','','customer-relationship','','K','','',1);
/*!40000 ALTER TABLE `tags` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `translate_html_logs`
--

DROP TABLE IF EXISTS `translate_html_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `translate_html_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `uri` varchar(190) NOT NULL DEFAULT '',
  `language` varchar(10) DEFAULT NULL,
  `to_language` varchar(10) DEFAULT NULL,
  `count` bigint(20) DEFAULT '0',
  `use_count` bigint(20) DEFAULT '0',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `remark` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `translate_html_logs`
--

LOCK TABLES `translate_html_logs` WRITE;
/*!40000 ALTER TABLE `translate_html_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `translate_html_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `translate_logs`
--

DROP TABLE IF EXISTS `translate_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `translate_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `md5` varchar(32) DEFAULT NULL,
  `origin_title` varchar(190) NOT NULL DEFAULT '',
  `origin_content` longtext,
  `title` varchar(190) NOT NULL DEFAULT '',
  `content` longtext,
  `language` varchar(10) DEFAULT NULL,
  `to_language` varchar(10) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_md5` (`md5`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `translate_logs`
--

LOCK TABLES `translate_logs` WRITE;
/*!40000 ALTER TABLE `translate_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `translate_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `translate_text_logs`
--

DROP TABLE IF EXISTS `translate_text_logs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `translate_text_logs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `md5` varchar(32) DEFAULT NULL,
  `language` varchar(10) DEFAULT NULL,
  `to_language` varchar(10) DEFAULT NULL,
  `text` text,
  `translated` text,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_md5` (`md5`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `translate_text_logs`
--

LOCK TABLES `translate_text_logs` WRITE;
/*!40000 ALTER TABLE `translate_text_logs` DISABLE KEYS */;
/*!40000 ALTER TABLE `translate_text_logs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `user_auths`
--

DROP TABLE IF EXISTS `user_auths`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `user_auths` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `type` tinyint(1) NOT NULL DEFAULT '1',
  `real_name` varchar(50) NOT NULL DEFAULT '',
  `id_card` varchar(32) NOT NULL DEFAULT '',
  `id_card_front` varchar(255) NOT NULL DEFAULT '',
  `id_card_back` varchar(255) NOT NULL DEFAULT '',
  `id_card_hand` varchar(255) NOT NULL DEFAULT '',
  `company_name` varchar(120) NOT NULL DEFAULT '',
  `company_no` varchar(64) NOT NULL DEFAULT '',
  `company_legal` varchar(50) NOT NULL DEFAULT '',
  `company_licence` varchar(255) NOT NULL DEFAULT '',
  `phone` varchar(20) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `remark` varchar(250) NOT NULL DEFAULT '',
  `audit_time` bigint(20) NOT NULL DEFAULT '0',
  `audit_admin_id` int(10) unsigned NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_auth_user_id` (`user_id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_auth_status` (`status`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `user_auths`
--

LOCK TABLES `user_auths` WRITE;
/*!40000 ALTER TABLE `user_auths` DISABLE KEYS */;
/*!40000 ALTER TABLE `user_auths` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `user_groups`
--

DROP TABLE IF EXISTS `user_groups`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `user_groups` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `title` varchar(32) NOT NULL DEFAULT '',
  `description` varchar(1000) NOT NULL DEFAULT '',
  `level` int(10) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `price` bigint(20) NOT NULL DEFAULT '0',
  `setting` text,
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=5 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `user_groups`
--

LOCK TABLES `user_groups` WRITE;
/*!40000 ALTER TABLE `user_groups` DISABLE KEYS */;
INSERT INTO `user_groups` (`id`, `created_time`, `updated_time`, `title`, `description`, `level`, `status`, `price`, `setting`) VALUES (1,1791860113,1792637224,'普通用户','',0,1,0,'{\"share_reward\":0,\"parent_reward\":0,\"share_discount\":0,\"discount\":0,\"shipping_discount\":0,\"expire_day\":0,\"content_no_verify\":false,\"content_no_captcha\":false}'),(2,1791860113,1792637224,'中级用户','',1,1,0,'{\"share_reward\":0,\"parent_reward\":0,\"share_discount\":0,\"discount\":0,\"shipping_discount\":0,\"expire_day\":0,\"content_no_verify\":false,\"content_no_captcha\":false}'),(3,1791860113,1792637224,'高级用户','',2,1,0,'{\"share_reward\":0,\"parent_reward\":0,\"share_discount\":0,\"discount\":0,\"shipping_discount\":0,\"expire_day\":0,\"content_no_verify\":false,\"content_no_captcha\":false}');
/*!40000 ALTER TABLE `user_groups` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `user_signs`
--

DROP TABLE IF EXISTS `user_signs`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `user_signs` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `sign_date` varchar(10) NOT NULL DEFAULT '',
  `days` int(10) NOT NULL DEFAULT '1',
  `points` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `idx_sign_user_date` (`user_id`,`sign_date`),
  KEY `idx_created_time` (`created_time`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `user_signs`
--

LOCK TABLES `user_signs` WRITE;
/*!40000 ALTER TABLE `user_signs` DISABLE KEYS */;
INSERT INTO `user_signs` (`id`, `created_time`, `user_id`, `sign_date`, `days`, `points`) VALUES (1,1792642123,1,'2026-10-22',1,5),(3,1792647232,2,'2026-10-22',1,5);
/*!40000 ALTER TABLE `user_signs` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `user_wechats`
--

DROP TABLE IF EXISTS `user_wechats`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `user_wechats` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `nickname` varchar(64) NOT NULL DEFAULT '',
  `avatar_url` varchar(255) NOT NULL DEFAULT '',
  `gender` int(10) NOT NULL DEFAULT '0',
  `openid` varchar(128) NOT NULL DEFAULT '',
  `union_id` varchar(128) NOT NULL DEFAULT '',
  `platform` varchar(32) NOT NULL DEFAULT '',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_user_wechats_user_id` (`user_id`),
  KEY `idx_user_wechats_openid` (`openid`),
  KEY `idx_user_wechats_union_id` (`union_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `user_wechats`
--

LOCK TABLES `user_wechats` WRITE;
/*!40000 ALTER TABLE `user_wechats` DISABLE KEYS */;
/*!40000 ALTER TABLE `user_wechats` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `user_withdraws`
--

DROP TABLE IF EXISTS `user_withdraws`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `user_withdraws` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `amount` bigint(20) NOT NULL DEFAULT '0',
  `success_time` bigint(20) DEFAULT '0',
  `withdraw_way` tinyint(1) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `error_times` bigint(20) NOT NULL DEFAULT '0',
  `last_time` bigint(20) NOT NULL DEFAULT '0',
  `remark` varchar(250) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_user_withdraws_user_id` (`user_id`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `user_withdraws`
--

LOCK TABLES `user_withdraws` WRITE;
/*!40000 ALTER TABLE `user_withdraws` DISABLE KEYS */;
INSERT INTO `user_withdraws` (`id`, `created_time`, `updated_time`, `user_id`, `amount`, `success_time`, `withdraw_way`, `status`, `error_times`, `last_time`, `remark`) VALUES (1,1793670540,1793670540,1,900,0,1,0,0,0,'');
/*!40000 ALTER TABLE `user_withdraws` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `users`
--

DROP TABLE IF EXISTS `users`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `users` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `parent_id` int(10) NOT NULL DEFAULT '0',
  `user_name` varchar(64) NOT NULL DEFAULT '',
  `url_token` varchar(190) NOT NULL DEFAULT '',
  `real_name` varchar(64) NOT NULL DEFAULT '',
  `first_name` varchar(32) NOT NULL DEFAULT '',
  `last_name` varchar(32) NOT NULL DEFAULT '',
  `birthday` bigint(20) DEFAULT '0',
  `avatar_url` varchar(255) NOT NULL DEFAULT '',
  `introduce` varchar(1000) NOT NULL DEFAULT '',
  `email` varchar(100) NOT NULL DEFAULT '',
  `email_verified` tinyint(1) NOT NULL DEFAULT '0',
  `phone` varchar(20) NOT NULL DEFAULT '',
  `group_id` int(10) unsigned NOT NULL DEFAULT '0',
  `google_id` varchar(255) NOT NULL DEFAULT '',
  `password` varchar(255) NOT NULL DEFAULT '',
  `reset_password` tinyint(1) NOT NULL DEFAULT '0',
  `status` tinyint(1) NOT NULL DEFAULT '0',
  `is_retailer` tinyint(1) NOT NULL DEFAULT '0',
  `balance` bigint(20) NOT NULL DEFAULT '0' COMMENT '''用户余额''',
  `total_reward` bigint(20) NOT NULL DEFAULT '0' COMMENT '''''',
  `invite_code` varchar(100) NOT NULL DEFAULT '',
  `last_login` bigint(20) DEFAULT '0',
  `expire_time` bigint(20) DEFAULT '0',
  `subscribed` tinyint(1) NOT NULL DEFAULT '0',
  `order_count` bigint(20) NOT NULL DEFAULT '0',
  `points` bigint(20) NOT NULL DEFAULT '0' COMMENT '''用户积分''',
  `api_token` varchar(64) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_users_url_token` (`url_token`),
  KEY `idx_email` (`email`),
  KEY `idx_users_phone` (`phone`),
  KEY `idx_invite_code` (`invite_code`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `users`
--

LOCK TABLES `users` WRITE;
/*!40000 ALTER TABLE `users` DISABLE KEYS */;
INSERT INTO `users` (`id`, `created_time`, `updated_time`, `parent_id`, `user_name`, `url_token`, `real_name`, `first_name`, `last_name`, `birthday`, `avatar_url`, `introduce`, `email`, `email_verified`, `phone`, `group_id`, `google_id`, `password`, `reset_password`, `status`, `is_retailer`, `balance`, `total_reward`, `invite_code`, `last_login`, `expire_time`, `subscribed`, `order_count`, `points`, `api_token`) VALUES (1,1792640628,1792640722,0,'member_test','member-test','测试会员','','',0,'uploads/user/1.png','这是会员中心的资料测试','member_test@example.com',0,'',1,'','$2a$12$EXqgF/bJXql2k2/imh7IpOPQZ6O6WIZU.c95uOvdTthEfUzoVLyvG',0,1,0,97000,0,'10l55fb',1792653929,0,0,0,5,''),(2,1792641148,1793751138,0,'yundayin','yundayin','','','',0,'uploads/user/2.png','','3936997014@qq.com',0,'13143242111',1,'','$2a$12$RkcZ872hHwr61QQIdYY5QOKbreM.ZMAUsXj7elX2BIxxMgD.c2FQm',0,1,0,0,0,'7g1ogd',1792653101,0,0,0,5,'');
/*!40000 ALTER TABLE `users` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `weapp_qrcodes`
--

DROP TABLE IF EXISTS `weapp_qrcodes`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `weapp_qrcodes` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `user_id` int(10) unsigned NOT NULL DEFAULT '0',
  `path` varchar(190) NOT NULL DEFAULT '',
  `code_url` varchar(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_weapp_qrcodes_user_id` (`user_id`),
  KEY `idx_path` (`path`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `weapp_qrcodes`
--

LOCK TABLES `weapp_qrcodes` WRITE;
/*!40000 ALTER TABLE `weapp_qrcodes` DISABLE KEYS */;
/*!40000 ALTER TABLE `weapp_qrcodes` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `websites`
--

DROP TABLE IF EXISTS `websites`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `websites` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `root_path` varchar(190) NOT NULL DEFAULT '',
  `name` varchar(128) NOT NULL DEFAULT '',
  `mysql` text,
  `token_secret` varchar(128) NOT NULL DEFAULT '',
  `status` tinyint(1) unsigned NOT NULL DEFAULT '0',
  `parent_id` int(10) NOT NULL DEFAULT '0',
  `sync_time` bigint(20) DEFAULT '0',
  `language_icon` varchar(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `websites`
--

LOCK TABLES `websites` WRITE;
/*!40000 ALTER TABLE `websites` DISABLE KEYS */;
/*!40000 ALTER TABLE `websites` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `wechat_menus`
--

DROP TABLE IF EXISTS `wechat_menus`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `wechat_menus` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `parent_id` int(10) NOT NULL DEFAULT '0',
  `name` varchar(20) NOT NULL DEFAULT '',
  `type` varchar(20) NOT NULL DEFAULT '',
  `value` varchar(255) NOT NULL DEFAULT '',
  `sort` int(10) NOT NULL DEFAULT '10',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_wechat_menus_sort` (`sort`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `wechat_menus`
--

LOCK TABLES `wechat_menus` WRITE;
/*!40000 ALTER TABLE `wechat_menus` DISABLE KEYS */;
/*!40000 ALTER TABLE `wechat_menus` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `wechat_messages`
--

DROP TABLE IF EXISTS `wechat_messages`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `wechat_messages` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `openid` varchar(128) NOT NULL DEFAULT '',
  `content` varchar(255) NOT NULL DEFAULT '',
  `reply` varchar(255) NOT NULL DEFAULT '',
  `reply_time` bigint(20) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`)
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `wechat_messages`
--

LOCK TABLES `wechat_messages` WRITE;
/*!40000 ALTER TABLE `wechat_messages` DISABLE KEYS */;
INSERT INTO `wechat_messages` (`id`, `created_time`, `updated_time`, `openid`, `content`, `reply`, `reply_time`) VALUES (1,1794132895,1794132895,'rustcms-route-openid','你好','',0);
/*!40000 ALTER TABLE `wechat_messages` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `wechat_reply_rules`
--

DROP TABLE IF EXISTS `wechat_reply_rules`;
/*!40101 SET @saved_cs_client     = @@character_set_client */;
/*!40101 SET character_set_client = utf8 */;
CREATE TABLE `wechat_reply_rules` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `created_time` bigint(20) DEFAULT NULL,
  `updated_time` bigint(20) DEFAULT NULL,
  `keyword` varchar(128) NOT NULL DEFAULT '',
  `content` text,
  `is_default` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `idx_created_time` (`created_time`),
  KEY `idx_updated_time` (`updated_time`),
  KEY `idx_wechat_reply_rules_keyword` (`keyword`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!40101 SET character_set_client = @saved_cs_client */;

--
-- Dumping data for table `wechat_reply_rules`
--

LOCK TABLES `wechat_reply_rules` WRITE;
/*!40000 ALTER TABLE `wechat_reply_rules` DISABLE KEYS */;
/*!40000 ALTER TABLE `wechat_reply_rules` ENABLE KEYS */;
UNLOCK TABLES;
/*!40103 SET TIME_ZONE=@OLD_TIME_ZONE */;

/*!40101 SET SQL_MODE=@OLD_SQL_MODE */;
/*!40014 SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS */;
/*!40014 SET UNIQUE_CHECKS=@OLD_UNIQUE_CHECKS */;
/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;
/*!40101 SET CHARACTER_SET_RESULTS=@OLD_CHARACTER_SET_RESULTS */;
/*!40101 SET COLLATION_CONNECTION=@OLD_COLLATION_CONNECTION */;
/*!40111 SET SQL_NOTES=@OLD_SQL_NOTES */;

-- Dump completed on 2026-11-09 13:43:31
